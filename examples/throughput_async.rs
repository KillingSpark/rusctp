use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::{Duration, Instant},
};

use bytes::{BufMut, Bytes, BytesMut};
use rand::RngCore;
use rusctp::{
    assoc::SendError,
    assoc_async::assoc::{AssociationRx, AssociationTx, Sctp},
    packet::{Chunk, Packet},
    Settings, TransportAddress,
};
use tokio::{net::UdpSocket, runtime::Runtime, task::JoinHandle};

fn main() {
    let mut args = std::env::args();
    args.next();
    let mut rt_server = None;
    let mut rt_client = None;

    let (signal_tx, signal_rx) = tokio::sync::broadcast::channel(1);

    let mode = args.next();
    match mode.as_ref().map(|x| x.as_str()) {
        Some("client") => {
            let client_addr = args
                .next()
                .unwrap_or("127.0.0.1:1338".to_owned())
                .parse()
                .unwrap();
            let server_addr = args
                .next()
                .unwrap_or("127.0.0.1:1337".to_owned())
                .parse()
                .unwrap();
            rt_client = Some(run_client(client_addr, server_addr, signal_rx));
        }
        Some("server") => {
            let server_addr = args
                .next()
                .unwrap_or("127.0.0.1:1337".to_owned())
                .parse()
                .unwrap();
            rt_server = Some(run_server(server_addr, signal_rx));
        }
        unknown => {
            eprintln!("{unknown:?}");

            let client_addr = args
                .next()
                .unwrap_or("127.0.0.1:1338".to_owned())
                .parse()
                .unwrap();
            let server_addr = args
                .next()
                .unwrap_or("127.0.0.1:1337".to_owned())
                .parse()
                .unwrap();
            rt_server = Some(run_server(server_addr, signal_tx.subscribe()));
            rt_client = Some(run_client(client_addr, server_addr, signal_rx));
        }
    }

    let signal_wait_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (rt_client, rt_server) = signal_wait_rt.block_on(async move {
        tokio::signal::ctrl_c().await.unwrap();
        signal_tx.send(()).unwrap();

        let (client_runtime, client_handle) = if let Some(x) = rt_client {
            (Some(x.0), Some(x.1))
        } else {
            (None, None)
        };
        let (server_runtime, server_handle) = if let Some(x) = rt_server {
            (Some(x.0), Some(x.1))
        } else {
            (None, None)
        };

        struct Join {
            server_handle: Option<JoinHandle<()>>,
            client_handle: Option<JoinHandle<()>>,
        }
        impl Future for Join {
            type Output = ();

            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                if let Some(handle) = self.server_handle.as_mut() {
                    if Pin::new(handle).poll(cx).is_ready() {
                        self.server_handle = None;
                    }
                }

                if let Some(handle) = self.client_handle.as_mut() {
                    if Pin::new(handle).poll(cx).is_ready() {
                        self.client_handle = None;
                    }
                }

                if self.server_handle.is_none() && self.client_handle.is_none() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }
        }
        tokio::select! {
            _ = Join{server_handle, client_handle} => {
                (client_runtime, server_runtime)
            },
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Got second ctrl-c, do harsh shutdown");
                if let Some(rt_client) = client_runtime {
                    rt_client.shutdown_background();
                }
                if let Some(rt_server) = server_runtime {
                    rt_server.shutdown_background();
                }
                (None, None)
            }
        }
    });

    eprintln!("Exit");
}

const PMTU: usize = 64_000;

fn make_settings() -> Settings {
    Settings {
        cookie_secret: b"oh boy a secret string".to_vec(),
        incoming_streams: 10,
        outgoing_streams: 10,
        in_buffer_limit: 1000 * PMTU,
        out_buffer_limit: 1000 * PMTU,
        pmtu: PMTU,
    }
}

fn run_client(
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    mut signal: tokio::sync::broadcast::Receiver<()>,
) -> (Runtime, JoinHandle<()>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let port = rand::thread_rng().next_u32() as u16;
    let sctp = Arc::new(Sctp::new(make_settings()));

    let handle = runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(client_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        let (stop_sctp_loops, mut done_signal) = ctx.sctp_network_loops();

        let assoc = ctx
            .sctp
            .connect(TransportAddress::Fake(server_addr), port, 200)
            .await;
        eprintln!("Client got assoc");
        let (tx, rx) = assoc.split();

        ctx.receive_data_loop(rx);
        ctx.network_send_loop(tx.clone());
        ctx.send_data_loop(tx.clone());

        // Wait for signal to shut down
        let _ = signal.recv().await;
        eprintln!("Start client shutdown");
        tx.initiate_shutdown();

        stop_sctp_loops.send(()).unwrap();
        done_signal.recv().await;
        eprintln!("Leave client");
    });
    (runtime, handle)
}

fn run_server(
    server_addr: SocketAddr,
    mut signal: tokio::sync::broadcast::Receiver<()>,
) -> (Runtime, JoinHandle<()>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let sctp = Arc::new(Sctp::new(make_settings()));

    let handle = runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(server_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        let (stop_sctp_loops, mut done_signal) = ctx.sctp_network_loops();

        loop {
            tokio::select! {
                assoc = ctx.sctp.accept() => {
                    eprintln!("Server got assoc");
                    let (tx, rx) = assoc.split();
                    ctx.receive_data_loop(rx);
                    ctx.network_send_loop(tx.clone());
                }
                _ = signal.recv() => {
                    break;
                }
            }
        }

        stop_sctp_loops.send(()).unwrap();
        done_signal.recv().await;
        // TODO cleanup all connections
        eprintln!("Leave server");
    });
    (runtime, handle)
}

struct Context {
    sctp: Arc<Sctp<SocketAddr>>,
    socket: Arc<UdpSocket>,
}

impl Context {
    fn sctp_network_loops(
        &self,
    ) -> (
        tokio::sync::broadcast::Sender<()>,
        tokio::sync::mpsc::Receiver<()>,
    ) {
        let (signal_tx, mut signal) = tokio::sync::broadcast::channel(1);
        let (signal_done_tx, signal_done_rx) = tokio::sync::mpsc::channel(1);
        let sctp = self.sctp.clone();
        let socket = self.socket.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    (addr, packet, chunk) = sctp.next_send_immediate() => {
                        if let TransportAddress::Fake(addr) = addr {
                            send_chunk(&socket, addr, packet, chunk).await.unwrap();
                        }
                    }
                }
            }
        })
        .abort_handle();

        let sctp = self.sctp.clone();
        let socket = self.socket.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; PMTU];
            let mut want_to_shutdown = false;
            loop {
                tokio::select! {
                    recv = socket.recv_from(&mut buf) => {
                        if let Ok((size, addr)) = recv {
                            sctp.receive_data(
                                Bytes::copy_from_slice(&buf[..size]),
                                TransportAddress::Fake(addr),
                            );
                            if want_to_shutdown && !sctp.has_assocs_left() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    _ = signal.recv() => {
                        want_to_shutdown = true;
                    }
                }
            }
            eprintln!("Left network receive loop");
            handle.abort();
            eprintln!("Aborted the network receive loop");
            signal_done_tx.send(()).await.unwrap();
        });
        (signal_tx, signal_done_rx)
    }

    fn network_send_loop(&self, tx: Arc<AssociationTx<SocketAddr>>) {
        let mut chunk_buf = BytesMut::with_capacity(PMTU - 100);
        let mut packet_buf = BytesMut::with_capacity(PMTU);
        let socket = self.socket.clone();
        tokio::spawn(async move {
            while !tx.shutdown_complete() {
                chunk_buf.clear();
                packet_buf.clear();
                let mut chunk_buf_limit = chunk_buf.limit(PMTU - 100);
                if let Some(timer) = tx.next_timeout() {
                    let timeout = timer.at() - Instant::now();
                    tokio::select! {
                        packet = collect_all_chunks(&tx, &mut chunk_buf_limit) => {
                            if let Ok(packet) = packet {
                                if let TransportAddress::Fake(addr) = tx.primary_path() {
                                    send_to(&socket, addr, packet, chunk_buf_limit.get_ref().as_ref(), &mut packet_buf).await.unwrap();
                                }
                            }
                        }
                        _ = tokio::time::sleep(timeout) => {
                            tx.handle_timeout(timer);
                        }
                    };
                } else {
                    if let Ok(packet) = collect_all_chunks(&tx, &mut chunk_buf_limit).await {
                        if let TransportAddress::Fake(addr) = tx.primary_path() {
                            send_to(
                                &socket,
                                addr,
                                packet,
                                chunk_buf_limit.get_ref().as_ref(),
                                &mut packet_buf,
                            )
                            .await
                            .unwrap();
                        }
                    }
                }
                chunk_buf = chunk_buf_limit.into_inner();
            }
            eprintln!("Left network send loop");
        });
    }

    fn send_data_loop(&self, tx: Arc<AssociationTx<SocketAddr>>) {
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; PMTU - 200]);
            let mut bytes_ctr = 0;
            let mut start = Instant::now();
            while !tx.shutdown_complete() {
                match tx.send_data(data.clone(), 0, 0, false, false).await {
                    Ok(()) => { /* happy */ }
                    Err(SendError { data: _, kind }) => {
                        eprintln!("Send errored: {:?}", kind);
                        break;
                    }
                }
                bytes_ctr += data.len() as u64;
                if start.elapsed() >= Duration::from_secs(1) {
                    let bytes_per_sec = (1_000_000 * bytes_ctr)
                        / (std::time::Instant::now() - start).as_micros() as u64;
                    format_throughput("Send", bytes_per_sec as usize);
                    start = std::time::Instant::now();
                    bytes_ctr = 0;
                }
            }
            eprintln!("Left send data loop");
        });
    }

    fn receive_data_loop(&self, rx: Arc<AssociationRx<SocketAddr>>) {
        tokio::spawn(async move {
            let mut bytes_ctr = 0u64;
            let mut start = std::time::Instant::now();
            loop {
                let Ok(data) = rx.recv_data(0).await else {
                    eprintln!("Receive errored");
                    break;
                };
                bytes_ctr += data.len() as u64;
                if start.elapsed() >= Duration::from_secs(1) {
                    let bytes_per_sec = (1_000_000 * bytes_ctr)
                        / (std::time::Instant::now() - start).as_micros() as u64;
                    format_throughput("Recv", bytes_per_sec as usize);
                    start = std::time::Instant::now();
                    bytes_ctr = 0;
                }
            }
            eprintln!("Left receive data loop");
        });
    }
}

async fn collect_all_chunks(
    tx: &Arc<AssociationTx<SocketAddr>>,
    chunks: &mut impl BufMut,
) -> Result<Packet, ()> {
    let (packet, chunk) = tx.poll_chunk_to_send(chunks.remaining_mut()).await?;
    chunk.serialize(chunks);
    while let Some((_, chunk)) = tx.try_poll_chunk_to_send(chunks.remaining_mut()).some() {
        chunk.serialize(chunks);
    }
    Ok(packet)
}

async fn send_to(
    socket: &UdpSocket,
    addr: SocketAddr,
    packet: Packet,
    chunkbuf: &[u8],
    buf: &mut BytesMut,
) -> Result<(), tokio::io::Error> {
    packet.serialize(buf, chunkbuf);
    buf.put_slice(&chunkbuf);

    socket.send_to(&buf, addr).await?;
    Ok(())
}

async fn send_chunk(
    socket: &UdpSocket,
    addr: SocketAddr,
    packet: Packet,
    chunk: Chunk<SocketAddr>,
) -> Result<(), tokio::io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    socket.send_to(&buf, addr).await?;
    Ok(())
}

fn format_throughput(place: &str, bytes_per_sec: usize) {
    if bytes_per_sec > 1_000_000_000 {
        eprintln!("{place} {:3} Gb/s", bytes_per_sec / 1_000_000_000);
    } else if bytes_per_sec > 1_000_000 {
        eprintln!("{place} {:3} Mb/s", bytes_per_sec / 1_000_000);
    } else {
        eprintln!("{place} {:3} b/s", bytes_per_sec);
    }
}
