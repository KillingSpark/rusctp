use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    task::Waker,
};

use bytes::Bytes;

use crate::{
    packet::{Chunk, Packet},
    AssocId, Settings, TransportAddress,
};

pub struct Sctp {
    inner: Arc<Mutex<InnerSctp>>,
}

struct InnerSctp {
    sctp: crate::Sctp,
    assocs: HashMap<AssocId, Association>,
    done: bool,
    ready_assocs: VecDeque<Association>,
    new_assoc_wakers: Vec<Waker>,
    send_immediate_wakers: Vec<Waker>,
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
    wrapped: Mutex<InnerTx>,
}

struct InnerTx {
    tx: crate::assoc::AssociationTx,
    send_wakers: Vec<Waker>,
    poll_wakers: Vec<Waker>,
}

pub struct AssociationRx {
    wrapped: Mutex<InnerRx>,
}

struct InnerRx {
    rx: crate::assoc::AssociationRx,
    recv_wakers: Vec<Waker>,
}

impl Sctp {
    pub fn new(settings: Settings) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerSctp {
                sctp: crate::Sctp::new(settings),
                done: false,
                assocs: HashMap::new(),
                ready_assocs: VecDeque::new(),
                new_assoc_wakers: Vec::new(),
                send_immediate_wakers: Vec::new(),
            })),
        }
    }

    pub fn receive_data(&self, data: Bytes, from: TransportAddress) {
        let mut inner = self.inner.lock().unwrap();
        inner.sctp.receive_data(data, from);
        if inner.sctp.has_next_send_immediate() {
            inner.send_immediate_wakers.drain(..).for_each(Waker::wake);
        }
    }

    pub fn connect(
        &self,
        peer_addr: TransportAddress,
        peer_port: u16,
        local_port: u16,
    ) -> impl Future<Output = Association> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .sctp
            .init_association(peer_addr, peer_port, local_port);

        struct ConnectFuture {
            inner: Arc<Mutex<InnerSctp>>,
        }
        impl Future for ConnectFuture {
            type Output = Association;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut inner = self.inner.lock().unwrap();
                if let Some(assoc) = inner.ready_assocs.pop_front() {
                    std::task::Poll::Ready(assoc)
                } else {
                    inner.new_assoc_wakers.push(cx.waker().to_owned());
                    std::task::Poll::Pending
                }
            }
        }
        ConnectFuture {
            inner: self.inner.clone(),
        }
    }

    pub fn accept(&self) -> impl Future<Output = Association> {
        struct AcceptFuture {
            inner: Arc<Mutex<InnerSctp>>,
        }
        impl Future for AcceptFuture {
            type Output = Association;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut inner = self.inner.lock().unwrap();
                if let Some(assoc) = inner.ready_assocs.pop_front() {
                    std::task::Poll::Ready(assoc)
                } else {
                    inner.new_assoc_wakers.push(cx.waker().to_owned());
                    std::task::Poll::Pending
                }
            }
        }
        AcceptFuture {
            inner: self.inner.clone(),
        }
    }

    pub fn kill(&self) {
        self.inner.lock().unwrap().done = true;
    }

    pub fn next_send_immediate(&self) -> impl Future<Output = (TransportAddress, Packet, Chunk)> {
        struct NextSendFuture {
            inner: Arc<Mutex<InnerSctp>>,
        }

        impl Future for NextSendFuture {
            type Output = (TransportAddress, Packet, Chunk);

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut inner = self.inner.lock().unwrap();
                if let Some(send) = inner.sctp.next_send_immediate() {
                    std::task::Poll::Ready(send)
                } else {
                    inner.send_immediate_wakers.push(cx.waker().to_owned());
                    std::task::Poll::Pending
                }
            }
        }

        NextSendFuture {
            inner: self.inner.clone(),
        }
    }

    pub fn handle_notifications(&self) {
        self.inner.lock().unwrap().handle_notifications()
    }
}

impl InnerSctp {
    fn handle_notifications(&mut self) {
        if let Some(assoc) = self.sctp.new_assoc() {
            let id = assoc.id();
            let (rx, tx) = assoc.split();
            let assoc = Association {
                tx: Arc::new(AssociationTx {
                    wrapped: Mutex::new(InnerTx {
                        tx,
                        send_wakers: vec![],
                        poll_wakers: vec![],
                    }),
                }),
                rx: Arc::new(AssociationRx {
                    wrapped: Mutex::new(InnerRx {
                        rx,
                        recv_wakers: vec![],
                    }),
                }),
            };
            self.assocs.insert(id, assoc.clone());
            self.ready_assocs.push_back(assoc);
            for waker in self.new_assoc_wakers.drain(..) {
                waker.wake();
            }
        }

        for (id, tx_notification) in self.sctp.tx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                tx.tx
                    .notification(tx_notification, std::time::Instant::now());
                tx.poll_wakers.drain(..).for_each(Waker::wake);
            }
        }

        for (id, rx_notification) in self.sctp.rx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                let mut rx = assoc.rx.wrapped.lock().unwrap();
                rx.rx
                    .notification(rx_notification, std::time::Instant::now());
                rx.recv_wakers.drain(..).for_each(Waker::wake);

                for tx_notification in rx.rx.tx_notifications() {
                    tx.tx
                        .notification(tx_notification, std::time::Instant::now());
                    tx.poll_wakers.drain(..).for_each(Waker::wake);
                }
            }
        }
    }
}

impl AssociationTx {
    pub fn send_data(
        self: &Arc<Self>,
        data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) -> impl Future<Output = ()> {
        struct SendFuture {
            tx: Arc<AssociationTx>,
            data: Option<Bytes>,
            stream: u16,
            ppid: u32,
            immediate: bool,
            unordered: bool,
        }
        impl Future for SendFuture {
            type Output = ();

            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let data = self.as_mut().data.take().unwrap();
                let mut wrapped = self.tx.wrapped.lock().unwrap();

                if let Some(returned) = wrapped.tx.try_send_data(
                    data,
                    self.stream,
                    self.ppid,
                    self.immediate,
                    self.unordered,
                ) {
                    wrapped.send_wakers.push(cx.waker().to_owned());
                    drop(wrapped);
                    self.data = Some(returned);
                    std::task::Poll::Pending
                } else {
                    for waker in wrapped.poll_wakers.drain(..) {
                        waker.wake();
                    }
                    std::task::Poll::Ready(())
                }
            }
        }

        SendFuture {
            tx: self.clone(),
            data: Some(data),
            stream,
            ppid,
            immediate,
            unordered,
        }
    }

    pub fn poll_chunk_to_send(self: &Arc<AssociationTx>) -> impl Future<Output = (Packet, Chunk)> {
        struct PollFuture {
            tx: Arc<AssociationTx>,
        }

        impl Future for PollFuture {
            type Output = (Packet, Chunk);

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut wrapped = self.tx.wrapped.lock().unwrap();
                if let Some(chunk) = wrapped
                    .tx
                    .poll_signal_to_send(1024)
                    .or_else(|| wrapped.tx.poll_data_to_send(1024).map(Chunk::Data))
                {
                    for waker in wrapped.send_wakers.drain(..) {
                        waker.wake();
                    }
                    std::task::Poll::Ready((wrapped.tx.packet_header(), chunk))
                } else {
                    wrapped.poll_wakers.push(cx.waker().to_owned());
                    std::task::Poll::Pending
                }
            }
        }

        PollFuture { tx: self.clone() }
    }
}

impl AssociationRx {
    pub fn recv_data(self: &Arc<AssociationRx>, stream: u16) -> impl Future<Output = Bytes> {
        struct RecvFuture {
            rx: Arc<AssociationRx>,
            stream: u16,
        }
        impl Future for RecvFuture {
            type Output = Bytes;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut wrapped = self.rx.wrapped.lock().unwrap();
                if let Some(data) = wrapped.rx.poll_data(self.stream) {
                    std::task::Poll::Ready(data)
                } else {
                    wrapped.recv_wakers.push(cx.waker().to_owned());
                    std::task::Poll::Pending
                }
            }
        }

        RecvFuture {
            rx: self.clone(),
            stream,
        }
    }
}
