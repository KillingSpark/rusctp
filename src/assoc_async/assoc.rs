use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
    time::Instant,
};

use bytes::Bytes;

use crate::{
    assoc::{PollDataError, PollDataResult, PollSendResult, SendError, SendErrorKind, Timer},
    packet::{Chunk, Packet},
    AssocId, FakeAddr, Settings, TransportAddress,
};

pub struct Sctp<FakeContent: FakeAddr> {
    inner: Arc<Mutex<InnerSctp<FakeContent>>>,
}

struct InnerSctp<FakeContent: FakeAddr> {
    sctp: crate::Sctp<FakeContent>,
    assocs: HashMap<AssocId, Association<FakeContent>>,
    ready_assocs: VecDeque<Association<FakeContent>>,
    new_assoc_wakers: Vec<Waker>,
    send_immediate_wakers: Vec<Waker>,
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
    wrapped: Mutex<InnerTx<FakeContent>>,
}

struct InnerTx<FakeContent: FakeAddr> {
    tx: crate::assoc::AssociationTx<FakeContent>,
    send_wakers: Vec<Waker>,
    poll_wakers: Vec<Waker>,
    shutdown_wakers: Vec<Waker>,
}

pub struct AssociationRx<FakeContent: FakeAddr> {
    wrapped: Mutex<InnerRx<FakeContent>>,
    tx: Arc<AssociationTx<FakeContent>>,
}

struct InnerRx<FakeContent: FakeAddr> {
    rx: crate::assoc::AssociationRx<FakeContent>,
    recv_wakers: HashMap<u16, Vec<Waker>>,
}

impl<FakeContent: FakeAddr> Sctp<FakeContent> {
    pub fn new(settings: Settings) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerSctp {
                sctp: crate::Sctp::new(settings),
                assocs: HashMap::new(),
                ready_assocs: VecDeque::new(),
                new_assoc_wakers: Vec::new(),
                send_immediate_wakers: Vec::new(),
            })),
        }
    }

    pub fn receive_data(&self, data: Bytes, from: TransportAddress<FakeContent>) {
        let mut inner = self.inner.lock().unwrap();
        inner.sctp.receive_data(data, from);
        if inner.sctp.has_next_send_immediate() {
            inner.send_immediate_wakers.drain(..).for_each(Waker::wake);
        }
        inner.handle_notifications();
    }

    pub fn connect(
        &self,
        peer_addr: TransportAddress<FakeContent>,
        peer_port: u16,
        local_port: u16,
    ) -> impl Future<Output = Association<FakeContent>> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .sctp
            .init_association(peer_addr, peer_port, local_port);

        struct ConnectFuture<FakeContent: FakeAddr> {
            inner: Arc<Mutex<InnerSctp<FakeContent>>>,
        }
        impl<FakeContent: FakeAddr> Future for ConnectFuture<FakeContent> {
            type Output = Association<FakeContent>;
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

    pub fn accept(&self) -> impl Future<Output = Association<FakeContent>> {
        struct AcceptFuture<FakeContent: FakeAddr> {
            inner: Arc<Mutex<InnerSctp<FakeContent>>>,
        }
        impl<FakeContent: FakeAddr> Future for AcceptFuture<FakeContent> {
            type Output = Association<FakeContent>;
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

    pub fn shutdown_all_connections(&self) {
        let inner = self.inner.lock().unwrap();
        inner.assocs.values().for_each(|assoc| {
            assoc.tx.initiate_shutdown();
        })
    }

    pub fn has_assocs_left(&self) -> bool {
        !self.inner.lock().unwrap().assocs.is_empty()
    }

    pub fn next_send_immediate(
        &self,
    ) -> impl Future<Output = (TransportAddress<FakeContent>, Packet, Chunk<FakeContent>)> {
        struct NextSendFuture<FakeContent: FakeAddr> {
            inner: Arc<Mutex<InnerSctp<FakeContent>>>,
        }

        impl<FakeContent: FakeAddr> Future for NextSendFuture<FakeContent> {
            type Output = (TransportAddress<FakeContent>, Packet, Chunk<FakeContent>);

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
}

impl<FakeContent: FakeAddr> InnerSctp<FakeContent> {
    fn handle_notifications(&mut self) {
        if let Some(assoc) = self.sctp.new_assoc() {
            let id = assoc.id();
            let (rx, tx) = assoc.split();
            let tx = Arc::new(AssociationTx {
                wrapped: Mutex::new(InnerTx {
                    tx,
                    send_wakers: vec![],
                    poll_wakers: vec![],
                    shutdown_wakers: vec![],
                }),
            });
            let assoc = Association {
                rx: Arc::new(AssociationRx {
                    wrapped: Mutex::new(InnerRx {
                        rx,
                        recv_wakers: HashMap::new(),
                    }),
                    tx: tx.clone(),
                }),
                tx,
            };
            self.assocs.insert(id, assoc.clone());
            self.ready_assocs.push_back(assoc);
            for waker in self.new_assoc_wakers.drain(..) {
                waker.wake();
            }
        }

        for (id, tx_notification) in self.sctp.tx_notifications() {
            let mut remove = false;
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut tx = assoc.tx.wrapped.lock().unwrap();
                tx.tx
                    .notification(tx_notification, std::time::Instant::now());
                tx.poll_wakers.drain(..).for_each(Waker::wake);
                if tx.tx.shutdown_complete() {
                    remove = true;
                }
            }
            if remove {
                // TODO notify sctp about this
                Self::remove_assoc(&mut self.assocs, id);
            }
        }

        for (id, rx_notification) in self.sctp.rx_notifications() {
            let mut remove = false;
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let mut rx = assoc.rx.wrapped.lock().unwrap();
                let stream_to_wake = rx_notification.get_stream_id();
                rx.rx
                    .notification(rx_notification, std::time::Instant::now());

                if let Some(wakers) = stream_to_wake.and_then(|stream_id| {
                    rx.recv_wakers
                        .get_mut(&stream_id)
                        .map(|wakers| wakers.drain(..))
                }) {
                    wakers.for_each(Waker::wake);
                }

                let mut tx = assoc.tx.wrapped.lock().unwrap();
                for tx_notification in rx.rx.tx_notifications() {
                    tx.tx
                        .notification(tx_notification, std::time::Instant::now());
                    tx.poll_wakers.drain(..).for_each(Waker::wake);
                }
                if tx.tx.shutdown_complete() {
                    remove = true;
                }
            }
            if remove {
                // TODO notify sctp about this
                Self::remove_assoc(&mut self.assocs, id);
            }
        }
    }

    fn remove_assoc(assocs: &mut HashMap<AssocId, Association<FakeContent>>, id: AssocId) {
        if let Some(assoc) = assocs.remove(&id) {
            let mut rx = assoc.rx.wrapped.lock().unwrap();
            rx.recv_wakers
                .iter_mut()
                .for_each(|(_, v)| v.drain(..).for_each(Waker::wake));
            let mut tx = assoc.tx.wrapped.lock().unwrap();
            tx.send_wakers.drain(..).for_each(Waker::wake);
            tx.poll_wakers.drain(..).for_each(Waker::wake);
            tx.shutdown_wakers.drain(..).for_each(Waker::wake);
        }
    }
}

impl<FakeContent: FakeAddr> AssociationTx<FakeContent> {
    pub fn send_data(
        self: &Arc<Self>,
        data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) -> impl Future<Output = Result<(), SendError>> {
        struct SendFuture<FakeContent: FakeAddr> {
            tx: Arc<AssociationTx<FakeContent>>,
            data: Option<Bytes>,
            stream: u16,
            ppid: u32,
            immediate: bool,
            unordered: bool,
        }
        impl<FakeContent: FakeAddr> Future for SendFuture<FakeContent> {
            type Output = Result<(), SendError>;

            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let data = self.as_mut().data.take().unwrap();
                let mut wrapped = self.tx.wrapped.lock().unwrap();

                match wrapped.tx.try_send_data(
                    data,
                    self.stream,
                    self.ppid,
                    self.immediate,
                    self.unordered,
                ) {
                    Ok(()) => {
                        wrapped.poll_wakers.drain(..).for_each(Waker::wake);
                        std::task::Poll::Ready(Ok(()))
                    }
                    Err(SendError {
                        data,
                        kind: SendErrorKind::BufferFull,
                    }) => {
                        //eprintln!("Send buffer full");
                        wrapped.send_wakers.push(cx.waker().to_owned());
                        drop(wrapped);
                        self.data = Some(data);
                        std::task::Poll::Pending
                    }
                    Err(err) => std::task::Poll::Ready(Err(err)),
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

    pub fn next_timeout(&self) -> Option<Timer> {
        self.wrapped.lock().unwrap().tx.next_timeout()
    }

    pub fn handle_timeout(&self, timer: Timer) {
        let mut wrapped = self.wrapped.lock().unwrap();
        wrapped.tx.handle_timeout(timer);
        wrapped.poll_wakers.drain(..).for_each(Waker::wake);
    }

    pub fn shutdown_complete(&self) -> bool {
        self.wrapped.lock().unwrap().tx.shutdown_complete()
    }

    pub fn await_shutdown(self: &Arc<AssociationTx<FakeContent>>) -> impl Future<Output = ()> {
        struct WaitForShutdownFuture<FakeContent: FakeAddr> {
            tx: Arc<AssociationTx<FakeContent>>,
        }

        impl<FakeContent: FakeAddr> Future for WaitForShutdownFuture<FakeContent> {
            type Output = ();
            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                if self.tx.shutdown_complete() {
                    Poll::Ready(())
                } else {
                    self.tx
                        .wrapped
                        .lock()
                        .unwrap()
                        .shutdown_wakers
                        .push(cx.waker().to_owned());
                    Poll::Pending
                }
            }
        }

        WaitForShutdownFuture { tx: self.clone() }
    }

    pub fn initiate_shutdown(&self) {
        self.wrapped.lock().unwrap().tx.initiate_shutdown();
    }

    pub fn poll_chunk_to_send(
        self: &Arc<AssociationTx<FakeContent>>,
        limit: usize,
    ) -> impl Future<Output = Result<(Packet, Chunk<FakeContent>), ()>> {
        struct PollFuture<FakeContent: FakeAddr> {
            tx: Arc<AssociationTx<FakeContent>>,
            limit: usize,
        }

        impl<FakeContent: FakeAddr> Future for PollFuture<FakeContent> {
            type Output = Result<(Packet, Chunk<FakeContent>), ()>;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut wrapped = self.tx.wrapped.lock().unwrap();
                let poll_result = AssociationTx::try_poll_any_chunk(&mut wrapped.tx, self.limit)
                    .map(|chunk| (wrapped.tx.packet_header(), chunk));
                match poll_result {
                    PollSendResult::Some(chunk) => {
                        if let Chunk::Data(_) = &chunk.1 {
                            for waker in wrapped.send_wakers.drain(..) {
                                waker.wake();
                            }
                        }
                        std::task::Poll::Ready(Ok(chunk))
                    }
                    PollSendResult::None => {
                        wrapped.poll_wakers.push(cx.waker().to_owned());
                        std::task::Poll::Pending
                    }
                    PollSendResult::Closed => {
                        for waker in wrapped.send_wakers.drain(..) {
                            waker.wake();
                        }

                        std::task::Poll::Ready(Err(()))
                    }
                }
            }
        }

        PollFuture {
            tx: self.clone(),
            limit,
        }
    }

    fn try_poll_any_chunk(
        tx: &mut crate::assoc::AssociationTx<FakeContent>,
        limit: usize,
    ) -> PollSendResult<Chunk<FakeContent>> {
        tx.poll_signal_to_send(limit, Instant::now())
            .or_else(|| tx.poll_data_to_send(limit, Instant::now()).map(Chunk::Data))
    }

    pub fn try_poll_chunk_to_send(
        self: &Arc<AssociationTx<FakeContent>>,
        limit: usize,
    ) -> PollSendResult<(Packet, Chunk<FakeContent>)> {
        let mut wrapped = self.wrapped.lock().unwrap();
        Self::try_poll_any_chunk(&mut wrapped.tx, limit)
            .map(|chunk| (wrapped.tx.packet_header(), chunk))
    }

    pub fn primary_path(&self) -> TransportAddress<FakeContent> {
        self.wrapped.lock().unwrap().tx.primary_path()
    }
}

impl<FakeContent: FakeAddr> AssociationRx<FakeContent> {
    pub fn recv_data(
        self: &Arc<AssociationRx<FakeContent>>,
        stream: u16,
    ) -> impl Future<Output = Result<Bytes, PollDataError>> {
        struct RecvFuture<FakeContent: FakeAddr> {
            rx: Arc<AssociationRx<FakeContent>>,
            stream: u16,
        }
        impl<FakeContent: FakeAddr> Future for RecvFuture<FakeContent> {
            type Output = Result<Bytes, PollDataError>;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut wrapped = self.rx.wrapped.lock().unwrap();
                match wrapped.rx.poll_data(self.stream) {
                    PollDataResult::Data(data) => {
                        let mut tx = self.rx.tx.wrapped.lock().unwrap();
                        for tx_notification in wrapped.rx.tx_notifications() {
                            tx.tx
                                .notification(tx_notification, std::time::Instant::now());
                            tx.poll_wakers.drain(..).for_each(Waker::wake);
                        }

                        std::task::Poll::Ready(Ok(data))
                    }
                    PollDataResult::NoneAvailable => {
                        wrapped
                            .recv_wakers
                            .entry(self.stream)
                            .or_default()
                            .push(cx.waker().to_owned());
                        std::task::Poll::Pending
                    }
                    PollDataResult::Error(err) => std::task::Poll::Ready(Err(err)),
                }
            }
        }

        RecvFuture {
            rx: self.clone(),
            stream,
        }
    }
}
