use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Condvar, Mutex},
};

use bytes::Bytes;

use crate::{
    packet::{Chunk, Packet},
    AssocId, Settings, TransportAddress,
};

pub struct Sctp<SendCb: Fn(Packet, Chunk) -> Result<(), io::Error> + Send + Sync> {
    inner: Arc<Mutex<InnerSctp<SendCb>>>,
    signal: Arc<Condvar>,
}

struct InnerSctp<SendCb: Fn(Packet, Chunk) -> Result<(), io::Error> + Send + Sync> {
    sctp: crate::Sctp,
    assocs: HashMap<AssocId, Association>,
    send_cb: HashMap<TransportAddress, SendCb>,
    done: bool,
    ready_assocs: VecDeque<Association>,
}

#[derive(Clone)]
pub struct Association {
    tx: Arc<AssociationTx>,
    rx: Arc<AssociationRx>,
}

impl Association {
    pub fn split(&self) -> (Arc<AssociationTx>, Arc<AssociationRx>) {
        (self.tx.clone(), self.rx.clone())
    }
}

pub struct AssociationTx {
    wrapped: Mutex<crate::assoc::AssociationTx>,
    signal: Condvar,
}
pub struct AssociationRx {
    wrapped: Mutex<crate::assoc::AssociationRx>,
    signal: Condvar,
}

impl<SendCb: Fn(Packet, Chunk) -> Result<(), io::Error> + Send + Sync + 'static> Sctp<SendCb> {
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

    pub fn register_address(&self, addr: TransportAddress, cb: SendCb) {
        self.inner.lock().unwrap().send_cb.insert(addr, cb);
    }

    pub fn receive_data(&self, data: Bytes, from: TransportAddress) {
        self.with_inner(|inner| {
            inner.sctp.receive_data(data, from);
            inner.handle_notifications();
        });
        self.signal.notify_all();
    }

    pub fn connect(
        &self,
        peer_addr: TransportAddress,
        peer_port: u16,
        local_port: u16,
    ) -> Association {
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

    pub fn accept(&self) -> Association {
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

    fn with_inner<Return>(&self, op: impl FnOnce(&mut InnerSctp<SendCb>) -> Return) -> Return {
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

impl<SendCb: Fn(Packet, Chunk) -> Result<(), io::Error> + Send + Sync> InnerSctp<SendCb> {
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
            eprintln!("Tx note: {tx_notification:?}");
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                tx.notification(tx_notification, std::time::Instant::now());
                assoc.tx.signal.notify_all();
            } else {
                eprintln!("dropped :(");
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

impl AssociationTx {
    pub fn send_data(
        &self,
        mut data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            if let Some(returned) = wrapped.try_send_data(data, stream, ppid, immediate, unordered)
            {
                data = returned;
            } else {
                self.signal.notify_all();
                break;
            }
            wrapped = self.signal.wait(wrapped).unwrap();
        }
    }

    pub fn poll_chunk_to_send(&self) -> (Packet, Chunk) {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            if let Some(chunk) = wrapped
                .poll_signal_to_send(1024)
                .or_else(|| wrapped.poll_data_to_send(1024).map(Chunk::Data))
            {
                return (wrapped.packet_header(), chunk);
            } else {
                wrapped = self.signal.wait(wrapped).unwrap();
            }
        }
    }
}

impl AssociationRx {
    pub fn recv_data(&self, stream: u16) -> Bytes {
        let mut wrapped = self.wrapped.lock().unwrap();
        loop {
            if let Some(data) = wrapped.poll_data(stream) {
                return data;
            }
            wrapped = self.signal.wait(wrapped).unwrap();
        }
    }
}
