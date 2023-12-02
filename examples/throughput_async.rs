use std::{
    net::SocketAddr,
    sync::Arc,
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
use tokio::{net::UdpSocket, runtime::Runtime};

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
    signal_wait_rt.block_on(async move {
        tokio::signal::ctrl_c().await.unwrap();
        signal_tx.send(()).unwrap();

        tokio::signal::ctrl_c().await.unwrap();
        eprintln!("Got second ctrl-c, do harsh shutdown");

        if let Some(rt_client) = rt_client {
            rt_client.shutdown_background();
        }
        if let Some(rt_server) = rt_server {
            rt_server.shutdown_background();
        }
    });
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
) -> Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let port = rand::thread_rng().next_u32() as u16;
    let sctp = Arc::new(Sctp::new(make_settings()));

    runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(client_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        let stop_sctp_loops = ctx.sctp_network_loops();

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
        tx.initiate_shutdown();

        stop_sctp_loops.send(()).unwrap();
    });
    runtime
}

fn run_server(
    server_addr: SocketAddr,
    mut signal: tokio::sync::broadcast::Receiver<()>,
) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let sctp = Arc::new(Sctp::new(make_settings()));

    runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(server_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        let stop_sctp_loops = ctx.sctp_network_loops();

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
        // TODO cleanup all connections
    });
    runtime
}

struct Context {
    sctp: Arc<Sctp<SocketAddr>>,
    socket: Arc<UdpSocket>,
}

impl Context {
    fn sctp_network_loops(&self) -> tokio::sync::broadcast::Sender<()> {
        let (signal_tx, mut signal) = tokio::sync::broadcast::channel(1);
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
        });
        signal_tx
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
            while !rx.shutdown_complete() {
                let data = rx.recv_data(0).await.unwrap();
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
