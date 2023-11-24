use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc_async::assoc::Sctp,
    packet::{Chunk, Packet},
    Settings,
};
use tokio::net::UdpSocket;

fn main() {
    let server_addr = "127.0.0.1:1337";
    let client_addr = "127.0.0.1:1338";

    let _rt_server = run_server(client_addr.parse().unwrap(), server_addr.parse().unwrap());
    let _rt_client = run_client(client_addr.parse().unwrap(), server_addr.parse().unwrap());

    loop {
        std::thread::sleep(Duration::from_secs(1000));
    }
}

fn run_client(client_addr: SocketAddr, server_addr: SocketAddr) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();
    let fake_addr = rusctp::TransportAddress::Fake(100);

    runtime.spawn(async move {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
            pmtu: 1500,
        }));

        let socket = Arc::new(UdpSocket::bind(client_addr).await.unwrap());
        socket.connect(server_addr).await.unwrap();

        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_to(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let mut buf = [0u8; 1024 * 100];
                    while let Ok(size) = socket.recv(&mut buf).await {
                        sctp.receive_data(Bytes::copy_from_slice(&buf[..size]), fake_addr);
                    }
                }
            });
        }
        let assoc = sctp.connect(fake_addr, 100, 200).await;
        eprintln!("Client got assoc");
        let (tx, rx) = assoc.split();

        let echo_tx = tx.clone();
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; 1400]);
            loop {
                echo_tx
                    .send_data(data.clone(), 0, 0, false, false)
                    .await
                    .unwrap()
            }
        });
        tokio::spawn(async move {
            loop {
                rx.recv_data(0).await.unwrap();
            }
        });
        loop {
            if let Some(timer) = tx.next_timeout() {
                let timeout = timer.at() - Instant::now();
                tokio::select! {
                    (packet, chunk) = tx.poll_chunk_to_send() => {
                        send_to(&socket, packet, chunk).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let (packet, chunk) = tx.poll_chunk_to_send().await;
                send_to(&socket, packet, chunk).await.unwrap();
            }
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
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
            pmtu: 1500,
        }));

        let socket = Arc::new(UdpSocket::bind(server_addr).await.unwrap());
        socket.connect(client_addr).await.unwrap();

        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_to(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let mut buf = [0u8; 1024 * 100];
                    while let Ok(size) = socket.recv(&mut buf).await {
                        sctp.receive_data(Bytes::copy_from_slice(&buf[..size]), fake_addr);
                    }
                }
            });
        }
        // Either wait for a connection or just initiate one with the same ports
        // let assoc = sctp.accept().await;
        let assoc = sctp.connect(fake_addr, 200, 100).await;
        eprintln!("Server got assoc");
        let (tx, rx) = assoc.split();
        let echo_tx = tx.clone();
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; 1400]);
            loop {
                echo_tx
                    .send_data(data.clone(), 0, 0, false, false)
                    .await
                    .unwrap()
            }
        });
        tokio::spawn(async move {
            let mut ctr = 1;
            let mut bytes_ctr = 0u64;
            let mut start = std::time::Instant::now();
            loop {
                let data = rx.recv_data(0).await.unwrap();
                bytes_ctr += data.len() as u64;
                ctr += 1;
                if ctr % 10000 == 0 {
                    eprintln!(
                        "{ctr} {}",
                        (1_000_000_ * bytes_ctr)
                            / (std::time::Instant::now() - start).as_micros() as u64
                    );
                    start = std::time::Instant::now();
                    bytes_ctr = 0;
                }
            }
        });
        loop {
            if let Some(timer) = tx.next_timeout() {
                let timeout = timer.at() - Instant::now();
                tokio::select! {
                    (packet, chunk) = tx.poll_chunk_to_send() => {
                        send_to(&socket, packet, chunk).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let (packet, chunk) = tx.poll_chunk_to_send().await;
                send_to(&socket, packet, chunk).await.unwrap();
            }
        }
    });
    runtime
}

async fn send_to(socket: &UdpSocket, packet: Packet, chunk: Chunk) -> Result<(), tokio::io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    socket.send(&buf).await?;
    Ok(())
}
