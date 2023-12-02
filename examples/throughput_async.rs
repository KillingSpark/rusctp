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
    let _rt_server;
    let _rt_client;

    let mode = args.next();
    match mode.as_ref().map(|x| x.as_str()) {
        Some("client") => {
            let server_addr = args.next().unwrap_or("127.0.0.1:1337".to_owned());
            let client_addr = args.next().unwrap_or("127.0.0.1:1338".to_owned());
            _rt_client = run_client(client_addr.parse().unwrap(), server_addr.parse().unwrap());
        }
        Some("server") => {
            let server_addr = args.next().unwrap_or("127.0.0.1:1337".to_owned());
            _rt_server = run_server(server_addr.parse().unwrap());
        }
        unknown => {
            eprintln!("{unknown:?}");

            let client_addr = args.next().unwrap_or("127.0.0.1:1338".to_owned());
            let server_addr = args.next().unwrap_or("127.0.0.1:1337".to_owned());
            _rt_server = run_server(server_addr.parse().unwrap());
            _rt_client = run_client(client_addr.parse().unwrap(), server_addr.parse().unwrap());
        }
    }

    loop {
        std::thread::sleep(Duration::from_secs(1000));
    }
}

const PMTU: usize = 64_000;
const PRINT_EVERY_X_BTES: u64 = 1_000_000_000;

struct Context {
    sctp: Arc<Sctp<SocketAddr>>,
    socket: Arc<UdpSocket>,
}

impl Context {
    fn sctp_network_loops(&self) {
        let sctp = self.sctp.clone();
        let socket = self.socket.clone();
        tokio::spawn(async move {
            loop {
                let (addr, packet, chunk) = sctp.next_send_immediate().await;
                if let TransportAddress::Fake(addr) = addr {
                    send_chunk(&socket, addr, packet, chunk).await.unwrap();
                }
            }
        });

        let sctp = self.sctp.clone();
        let socket = self.socket.clone();
        tokio::spawn(async move {
            loop {
                let mut buf = [0u8; PMTU];
                while let Ok((size, addr)) = socket.recv_from(&mut buf).await {
                    sctp.receive_data(
                        Bytes::copy_from_slice(&buf[..size]),
                        TransportAddress::Fake(addr),
                    );
                }
            }
        });
    }

    fn network_send_loop(&self, tx: Arc<AssociationTx<SocketAddr>>) {
        let mut chunk_buf = BytesMut::with_capacity(PMTU - 100);
        let mut packet_buf = BytesMut::with_capacity(PMTU);
        let socket = self.socket.clone();
        tokio::spawn(async move {
            loop {
                chunk_buf.clear();
                packet_buf.clear();
                let mut chunk_buf_limit = chunk_buf.limit(PMTU - 100);
                if let Some(timer) = tx.next_timeout() {
                    let timeout = timer.at() - Instant::now();

                    tokio::select! {
                        packet = collect_all_chunks(&tx, &mut chunk_buf_limit) => {
                            if let TransportAddress::Fake(addr) = tx.primary_path() {
                                send_to(&socket, addr, packet, chunk_buf_limit.get_ref().as_ref(), &mut packet_buf).await.unwrap();
                            }
                        }
                        _ = tokio::time::sleep(timeout) => {
                            tx.handle_timeout(timer);
                        }
                    };
                } else {
                    let packet = collect_all_chunks(&tx, &mut chunk_buf_limit).await;
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
                chunk_buf = chunk_buf_limit.into_inner();
            }
        });
    }

    fn send_data_loop(&self, tx: Arc<AssociationTx<SocketAddr>>) {
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; PMTU - 200]);
            let mut bytes_ctr = 0;
            let mut start = Instant::now();
            loop {
                match tx.send_data(data.clone(), 0, 0, false, false).await {
                    Ok(()) => { /* happy */ }
                    Err(SendError { data: _, kind }) => {
                        eprintln!("Send errored: {:?}", kind);
                        break;
                    }
                }
                bytes_ctr += data.len() as u64;
                if bytes_ctr >= PRINT_EVERY_X_BTES {
                    let bytes_per_sec = (1_000_000 * bytes_ctr)
                        / (std::time::Instant::now() - start).as_micros() as u64;
                    format_throughput("Send", bytes_per_sec as usize);
                    start = std::time::Instant::now();
                    bytes_ctr = 0;
                }
            }
        });
    }

    fn receive_data_loop(&self, rx: Arc<AssociationRx<SocketAddr>>) {
        tokio::spawn(async move {
            let mut bytes_ctr = 0u64;
            let mut start = std::time::Instant::now();
            loop {
                let data = rx.recv_data(0).await.unwrap();
                bytes_ctr += data.len() as u64;
                if bytes_ctr >= PRINT_EVERY_X_BTES {
                    let bytes_per_sec = (1_000_000 * bytes_ctr)
                        / (std::time::Instant::now() - start).as_micros() as u64;
                    format_throughput("Recv", bytes_per_sec as usize);
                    start = std::time::Instant::now();
                    bytes_ctr = 0;
                }
            }
        });
    }
}

fn run_client(client_addr: SocketAddr, server_addr: SocketAddr) -> Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let port = rand::thread_rng().next_u32() as u16;
    let sctp = Arc::new(Sctp::new(Settings {
        cookie_secret: b"oh boy a secret string".to_vec(),
        incoming_streams: 10,
        outgoing_streams: 10,
        in_buffer_limit: 1000 * PMTU,
        out_buffer_limit: 1000 * PMTU,
        pmtu: PMTU,
    }));

    runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(client_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        ctx.sctp_network_loops();

        let assoc = ctx
            .sctp
            .connect(TransportAddress::Fake(server_addr), port, 200)
            .await;
        eprintln!("Client got assoc");
        let (tx, rx) = assoc.split();

        ctx.receive_data_loop(rx);
        ctx.network_send_loop(tx.clone());
        ctx.send_data_loop(tx.clone());

        //tx.initiate_shutdown();
    });
    runtime
}

fn run_server(server_addr: SocketAddr) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let sctp = Arc::new(Sctp::new(Settings {
        cookie_secret: b"oh boy a secret string".to_vec(),
        incoming_streams: 10,
        outgoing_streams: 10,
        in_buffer_limit: 100 * PMTU,
        out_buffer_limit: 100 * PMTU,
        pmtu: PMTU,
    }));

    runtime.spawn(async move {
        let socket = Arc::new(UdpSocket::bind(server_addr).await.unwrap());

        let ctx = Context { sctp, socket };

        ctx.sctp_network_loops();

        loop {
            let assoc = ctx.sctp.accept().await;
            eprintln!("Server got assoc");
            let (tx, rx) = assoc.split();

            ctx.receive_data_loop(rx);
            ctx.network_send_loop(tx.clone());
        }
    });
    runtime
}

async fn collect_all_chunks(
    tx: &Arc<AssociationTx<SocketAddr>>,
    chunks: &mut impl BufMut,
) -> Packet {
    let (packet, chunk) = tx.poll_chunk_to_send(chunks.remaining_mut()).await;
    chunk.serialize(chunks);
    while let Some((_, chunk)) = tx.try_poll_chunk_to_send(chunks.remaining_mut()) {
        chunk.serialize(chunks);
    }
    packet
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
