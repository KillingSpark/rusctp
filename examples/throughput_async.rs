use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::{BufMut, Bytes, BytesMut};
use rand::RngCore;
use rusctp::{
    assoc_async::assoc::{AssociationTx, Sctp},
    packet::{Chunk, Packet},
    Settings,
};
use tokio::net::UdpSocket;

fn main() {
    let mut args = std::env::args();
    args.next();
    let _rt_server;
    let _rt_client;

    let mode = args.next();
    let client_addr = args.next().unwrap_or("127.0.0.1:1338".to_owned());
    let server_addr = args.next().unwrap_or("127.0.0.1:1337".to_owned());
    match mode.as_ref().map(|x| x.as_str()) {
        Some("client") => {
            _rt_client = run_client(client_addr.parse().unwrap(), server_addr.parse().unwrap());
        }
        Some("server") => {
            _rt_client = run_server(client_addr.parse().unwrap(), server_addr.parse().unwrap());
        }
        unknown => {
            eprintln!("{unknown:?}");

            _rt_server = run_server(client_addr.parse().unwrap(), server_addr.parse().unwrap());
            _rt_client = run_client(client_addr.parse().unwrap(), server_addr.parse().unwrap());
        }
    }

    loop {
        std::thread::sleep(Duration::from_secs(1000));
    }
}

const PMTU: usize = 64_000;
const PRINT_EVERY_X_BTES: u64 = 1_000_000_000;

fn run_client(client_addr: SocketAddr, server_addr: SocketAddr) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let fake_addr = rusctp::TransportAddress::Fake(100);
    let port = rand::thread_rng().next_u32() as u16;
    runtime.spawn(async move {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 1000 * PMTU,
            out_buffer_limit: 1000 * PMTU,
            pmtu: PMTU,
        }));

        let socket = Arc::new(UdpSocket::bind(client_addr).await.unwrap());
        socket.connect(server_addr).await.unwrap();

        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_chunk(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let mut buf = [0u8; PMTU];
                    while let Ok(size) = socket.recv(&mut buf).await {
                        sctp.receive_data(Bytes::copy_from_slice(&buf[..size]), fake_addr);
                    }
                }
            });
        }
        let assoc = sctp.connect(fake_addr, port, 200).await;
        eprintln!("Client got assoc");
        let (tx, rx) = assoc.split();

        let echo_tx = tx.clone();
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; PMTU - 200]);
            let mut bytes_ctr = 0;
            let mut start = Instant::now();
            loop {
                echo_tx
                    .send_data(data.clone(), 0, 0, false, false)
                    .await
                    .unwrap();
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
        tokio::spawn(async move {
            loop {
                rx.recv_data(0).await.unwrap();
            }
        });
        let mut chunk_buf = BytesMut::with_capacity(PMTU - 100);
        let mut packet_buf = BytesMut::with_capacity(PMTU);
        loop {
            chunk_buf.clear();
            packet_buf.clear();
            let mut chunk_buf_limit = chunk_buf.limit(PMTU - 100);
            if let Some(timer) = tx.next_timeout() {
                let timeout = timer.at() - Instant::now();

                tokio::select! {
                    packet = collect_all_chunks(&tx, &mut chunk_buf_limit) => {
                        send_to(&socket, packet, chunk_buf_limit.get_ref().as_ref(), &mut packet_buf).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let packet = collect_all_chunks(&tx, &mut chunk_buf_limit).await;

                send_to(
                    &socket,
                    packet,
                    chunk_buf_limit.get_ref().as_ref(),
                    &mut packet_buf,
                )
                .await
                .unwrap();
            }
            chunk_buf = chunk_buf_limit.into_inner();
        }
    });
    runtime
}

fn run_server(client_addr: SocketAddr, server_addr: SocketAddr) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();

    let fake_addr = rusctp::TransportAddress::Fake(200);

    runtime.spawn(async move {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * PMTU,
            out_buffer_limit: 100 * PMTU,
            pmtu: PMTU,
        }));

        let socket = Arc::new(UdpSocket::bind(server_addr).await.unwrap());
        socket.connect(client_addr).await.unwrap();

        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_chunk(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let mut buf = [0u8; PMTU];
                    while let Ok(size) = socket.recv(&mut buf).await {
                        sctp.receive_data(Bytes::copy_from_slice(&buf[..size]), fake_addr);
                    }
                }
            });
        }

        loop {
            let assoc = sctp.accept().await;
            let socket = socket.clone();
            tokio::spawn(async move {
                eprintln!("Server got assoc");
                let (tx, rx) = assoc.split();
                //let echo_tx = tx.clone();
                //tokio::spawn(async move {
                //    let data = Bytes::copy_from_slice(&[0u8; PMTU - 200]);
                //    loop {
                //        echo_tx
                //            .send_data(data.clone(), 0, 0, false, false)
                //            .await
                //            .unwrap()
                //    }
                //});
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
                let mut chunk_buf = BytesMut::with_capacity(PMTU - 100);
                let mut packet_buf = BytesMut::with_capacity(PMTU);
                loop {
                    chunk_buf.clear();
                    packet_buf.clear();
                    let mut chunk_buf_limit = chunk_buf.limit(PMTU - 100);
                    if let Some(timer) = tx.next_timeout() {
                        let timeout = timer.at() - Instant::now();
                        tokio::select! {
                            packet = collect_all_chunks(&tx, &mut chunk_buf_limit) => {
                                send_to(&socket, packet, chunk_buf_limit.get_ref().as_ref(), &mut packet_buf).await.unwrap();
                            }
                            _ = tokio::time::sleep(timeout) => {
                                tx.handle_timeout(timer);
                            }
                        };
                    } else {
                        let packet = collect_all_chunks(&tx, &mut chunk_buf_limit).await;
                        send_to(
                            &socket,
                            packet,
                            chunk_buf_limit.get_ref().as_ref(),
                            &mut packet_buf,
                        )
                        .await
                        .unwrap();
                    }
                    chunk_buf = chunk_buf_limit.into_inner();
                }
            });
        }
    });
    runtime
}

async fn collect_all_chunks(tx: &Arc<AssociationTx>, chunks: &mut impl BufMut) -> Packet {
    let (packet, chunk) = tx.poll_chunk_to_send(chunks.remaining_mut()).await;
    chunk.serialize(chunks);
    while let Some((_, chunk)) = tx.try_poll_chunk_to_send(chunks.remaining_mut()) {
        chunk.serialize(chunks);
    }
    packet
}

async fn send_to(
    socket: &UdpSocket,
    packet: Packet,
    chunkbuf: &[u8],
    buf: &mut BytesMut,
) -> Result<(), tokio::io::Error> {
    packet.serialize(buf, chunkbuf);
    buf.put_slice(&chunkbuf);

    socket.send(&buf).await?;
    Ok(())
}

async fn send_chunk(
    socket: &UdpSocket,
    packet: Packet,
    chunk: Chunk,
) -> Result<(), tokio::io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    socket.send(&buf).await?;
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
