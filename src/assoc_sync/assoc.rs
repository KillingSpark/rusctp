use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};

use bytes::Bytes;

use crate::{
    assoc::{PollDataError, PollDataResult, PollSendResult, SendError, SendErrorKind},
    packet::{Chunk, Packet},
    AssocId, FakeAddr, Settings, TransportAddress,
};

pub struct Sctp<
    FakeContent: FakeAddr,
    SendCb: Fn(Packet, Chunk<FakeContent>) -> Result<(), io::Error> + Send + Sync,
> {
    inner: Arc<Mutex<InnerSctp<FakeContent, SendCb>>>,
    signal: Arc<Condvar>,
}

struct InnerSctp<
    FakeContent: FakeAddr,
    SendCb: Fn(Packet, Chunk<FakeContent>) -> Result<(), io::Error> + Send + Sync,
> {
    sctp: crate::Sctp<FakeContent>,
    assocs: HashMap<AssocId, Association<FakeContent>>,
    send_cb: HashMap<TransportAddress<FakeContent>, SendCb>,
    done: bool,
    ready_assocs: VecDeque<Association<FakeContent>>,
}

#[derive(Clone)]
pub struct Association<FakeContent: FakeAddr> {
    tx: Arc<AssociationTx<FakeContent>>,
    rx: Arc<AssociationRx<FakeContent>>,
}

impl<FakeContent: FakeAddr> Association<FakeContent> {
    pub fn split(
        &self,
    ) -> (
        Arc<AssociationTx<FakeContent>>,
        Arc<AssociationRx<FakeContent>>,
    ) {
        (self.tx.clone(), self.rx.clone())
    }
}

pub struct AssociationTx<FakeContent: FakeAddr> {
    wrapped: Mutex<crate::assoc::AssociationTx<FakeContent>>,
    signal: Condvar,
}
pub struct AssociationRx<FakeContent: FakeAddr> {
    wrapped: Mutex<crate::assoc::AssociationRx<FakeContent>>,
    signal: Condvar,
}

impl<
        FakeContent: FakeAddr + Send + Sync,
        SendCb: Fn(Packet, Chunk<FakeContent>) -> Result<(), io::Error> + Send + Sync + 'static,
    > Sctp<FakeContent, SendCb>
{
    pub fn new(settings: Settings) -> Self {
        let this = Self {
            inner: Arc::new(Mutex::new(InnerSctp {
                sctp: crate::Sctp::new(settings),
                done: false,
                assocs: HashMap::new(),
                send_cb: HashMap::new(),
                ready_assocs: VecDeque::new(),
            })),
            signal: Default::default(),
        };
        let this2 = Self {
            inner: this.inner.clone(),
            signal: this.signal.clone(),
        };

        std::thread::spawn(move || {
            Self::main_loop(this);
        });

        this2
    }

    pub fn register_address(&self, addr: TransportAddress<FakeContent>, cb: SendCb) {
        self.inner.lock().unwrap().send_cb.insert(addr, cb);
    }

    pub fn receive_data(&self, data: Bytes, from: TransportAddress<FakeContent>) {
        self.with_inner(|inner| {
            inner.sctp.receive_data(data, from);
            inner.handle_notifications();
        });
        self.signal.notify_all();
    }

    pub fn connect(
        &self,
        peer_addr: TransportAddress<FakeContent>,
        peer_port: u16,
        local_port: u16,
    ) -> Association<FakeContent> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .sctp
            .init_association(peer_addr, peer_port, local_port);
        self.signal.notify_all();
        loop {
            if let Some(assoc) = inner.ready_assocs.pop_front() {
                return assoc;
            }
            inner = self.signal.wait(inner).unwrap();
        }
    }

    pub fn accept(&self) -> Association<FakeContent> {
        let mut inner = self.inner.lock().unwrap();
        loop {
            if let Some(assoc) = inner.ready_assocs.pop_front() {
                return assoc;
            }
            inner = self.signal.wait(inner).unwrap();
        }
    }

    pub fn kill(&self) {
        self.inner.lock().unwrap().done = true;
        self.signal.notify_all();
    }

    fn with_inner<Return>(
        &self,
        op: impl FnOnce(&mut InnerSctp<FakeContent, SendCb>) -> Return,
    ) -> Return {
        (op)(&mut self.inner.lock().unwrap())
    }

    fn main_loop(self) {
        let mut inner = self.inner.lock().unwrap();
        'kill: loop {
            if inner.done {
                break 'kill;
            }
            inner.handle_notifications();
            inner = self.signal.wait(inner).unwrap();
        }
    }
}

impl<
        FakeContent: FakeAddr,
        SendCb: Fn(Packet, Chunk<FakeContent>) -> Result<(), io::Error> + Send + Sync,
    > InnerSctp<FakeContent, SendCb>
{
    fn handle_notifications(&mut self) {
        if let Some(assoc) = self.sctp.new_assoc() {
            let id = assoc.id();
            let (rx, tx) = assoc.split();
            let assoc = Association {
                tx: Arc::new(AssociationTx {
                    wrapped: Mutex::new(tx),
                    signal: Condvar::default(),
                }),
                rx: Arc::new(AssociationRx {
                    wrapped: Mutex::new(rx),
                    signal: Condvar::default(),
                }),
            };
            self.assocs.insert(id, assoc.clone());
            self.ready_assocs.push_back(assoc);
        }

        for (to, packet, chunk) in self.sctp.send_immediate() {
            if let Some(cb) = self.send_cb.get(&to) {
                cb(packet, chunk).unwrap();
            }
        }

        for (id, tx_notification) in self.sctp.tx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                tx.notification(tx_notification, std::time::Instant::now());
                assoc.tx.signal.notify_all();
            }
        }

        for (id, rx_notification) in self.sctp.rx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                let mut rx = assoc.rx.wrapped.lock().unwrap();
                rx.notification(rx_notification, std::time::Instant::now());
                assoc.rx.signal.notify_all();

                for tx_notification in rx.tx_notifications() {
                    tx.notification(tx_notification, std::time::Instant::now());
                    assoc.tx.signal.notify_all();
                }
            }
        }
    }
}

impl<FakeContent: FakeAddr> AssociationTx<FakeContent> {
    pub fn send_data(
        &self,
        mut data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) -> Result<(), SendError> {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            match wrapped.try_send_data(data, stream, ppid, immediate, unordered) {
                Ok(()) => {
                    self.signal.notify_all();
                    break Ok(());
                }
                Err(SendError {
                    data: returned,
                    kind: SendErrorKind::BufferFull,
                }) => {
                    data = returned;
                    wrapped = self.signal.wait(wrapped).unwrap();
                }
                Err(err) => {
                    break Err(err);
                }
            }
        }
    }

    pub fn poll_chunk_to_send(&self) -> (Packet, Chunk<FakeContent>) {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            match wrapped
                .poll_signal_to_send(1024, Instant::now())
                .or_else(|| {
                    wrapped
                        .poll_data_to_send(1024, Instant::now())
                        .map(Chunk::Data)
                }) {
                PollSendResult::Some(chunk) => {
                    return (wrapped.packet_header(), chunk);
                }
                PollSendResult::Closed => {
                    panic!("Closed");
                }
                PollSendResult::None => {
                    if let Some(timeout) = wrapped.next_timeout() {
                        let res = self
                            .signal
                            .wait_timeout(wrapped, Instant::now() - timeout.at())
                            .unwrap();
                        wrapped = res.0;
                        if res.1.timed_out() {
                            wrapped.handle_timeout(timeout);
                        }
                    } else {
                        wrapped = self.signal.wait(wrapped).unwrap();
                    }
                }
            }
        }
    }
}

impl<FakeContent: FakeAddr> AssociationRx<FakeContent> {
    pub fn recv_data(&self, stream: u16) -> Result<Bytes, PollDataError> {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            match wrapped.poll_data(stream) {
                PollDataResult::NoneAvailable => wrapped = self.signal.wait(wrapped).unwrap(),
                PollDataResult::Data(data) => return Ok(data),
                PollDataResult::Error(err) => return Err(err),
            }
        }
    }
}
