use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc_async::assoc::Sctp,
    packet::{Chunk, Packet},
    FakeAddr, Settings,
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
    let fake_addr = rusctp::TransportAddress::Fake(100u64);

    runtime.spawn(async move {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
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

        tx.send_data(Bytes::copy_from_slice(&b"Coolio"[..]), 0, 0, false, false)
            .await
            .unwrap();

        let echo_tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let data = rx.recv_data().await.unwrap();
                for chunk in data.data {
                    echo_tx
                        .send_data(chunk.buf, 0, 0, false, false)
                        .await
                        .unwrap();
                }
            }
        });
        loop {
            if let Some(timer) = tx.next_timeout() {
                let timeout = timer.at() - Instant::now();
                tokio::select! {
                    poll_result = tx.poll_chunk_to_send(1500,0) => {
                        let (packet, chunk) = poll_result.unwrap();
                        send_to(&socket, packet, chunk).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let (packet, chunk) = tx.poll_chunk_to_send(1500, 0).await.unwrap();
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

    let fake_addr = rusctp::TransportAddress::Fake(200u64);

    runtime.spawn(async move {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
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
        let assoc = sctp.accept().await;
        eprintln!("Server got assoc");
        let (tx, rx) = assoc.split();
        let echo_tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let data = rx.recv_data().await.unwrap();
                for chunk in data.data {
                    echo_tx
                        .send_data(chunk.buf, 0, 0, false, false)
                        .await
                        .unwrap();
                }
            }
        });
        loop {
            if let Some(timer) = tx.next_timeout() {
                let timeout = timer.at() - Instant::now();
                tokio::select! {
                    poll_result = tx.poll_chunk_to_send(1500,0) => {
                        let (packet, chunk) = poll_result.unwrap();
                        send_to(&socket, packet, chunk).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let (packet, chunk) = tx.poll_chunk_to_send(1500, 0).await.unwrap();
                send_to(&socket, packet, chunk).await.unwrap();
            }
        }
    });
    runtime
}

async fn send_to<F: FakeAddr>(
    socket: &UdpSocket,
    packet: Packet,
    chunk: Chunk<F>,
) -> Result<(), tokio::io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    eprintln!("Send {packet:?} {chunk:?}");
    socket.send(&buf).await?;
    Ok(())
}
