use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc_async::assoc::{AssociationTx, Sctp},
    packet::{Chunk, Packet},
    FakeAddr, Settings,
};
use tokio::sync::mpsc::{Receiver, Sender};

fn main() {
    let (server_tx, server_rx) = tokio::sync::mpsc::channel(1_000_000);
    let (client_tx, client_rx) = tokio::sync::mpsc::channel(1_000_000);

    let _rt_server = run_server(server_tx, client_rx);
    let _rt_client = run_client(client_tx, server_rx);

    loop {
        std::thread::sleep(Duration::from_secs(1000));
    }
}

const PMTU: usize = 64_000;

fn run_client(
    transport_tx: Sender<Bytes>,
    mut transport_rx: Receiver<Bytes>,
) -> tokio::runtime::Runtime {
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
            in_buffer_limit: 1000 * PMTU,
            out_buffer_limit: 1000 * PMTU,
        }));

        {
            let sctp = sctp.clone();
            let socket = transport_tx.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_chunk(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            tokio::spawn(async move {
                loop {
                    while let Some(data) = transport_rx.recv().await {
                        sctp.receive_data(data, fake_addr);
                    }
                }
            });
        }
        let assoc = sctp.connect(fake_addr, 100, 200).await;
        eprintln!("Client got assoc");
        let (tx, rx) = assoc.split();

        let echo_tx = tx.clone();
        tokio::spawn(async move {
            let data = Bytes::copy_from_slice(&[0u8; PMTU - 200]);
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

                let mut chunk_buf = BytesMut::with_capacity(PMTU - 100).limit(PMTU - 100);
                tokio::select! {
                    packet = collect_all_chunks(&tx, &mut chunk_buf) => {
                        send_to(&transport_tx, packet.unwrap(), chunk_buf.into_inner().freeze()).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let mut chunk_buf = BytesMut::with_capacity(PMTU - 100).limit(PMTU - 100);
                let packet = collect_all_chunks(&tx, &mut chunk_buf).await;

                send_to(&transport_tx, packet.unwrap(), chunk_buf.into_inner().freeze())
                    .await
                    .unwrap();
            }
        }
    });
    runtime
}

fn run_server(
    transport_tx: Sender<Bytes>,
    mut transport_rx: Receiver<Bytes>,
) -> tokio::runtime::Runtime {
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
            in_buffer_limit: 1000 * PMTU,
            out_buffer_limit: 1000 * PMTU,
        }));

        {
            let sctp = sctp.clone();
            let socket = transport_tx.clone();
            tokio::spawn(async move {
                loop {
                    let (_, packet, chunk) = sctp.next_send_immediate().await;
                    send_chunk(&socket, packet, chunk).await.unwrap();
                }
            });
        }
        {
            let sctp = sctp.clone();
            tokio::spawn(async move {
                loop {
                    while let Some(data) = transport_rx.recv().await {
                        sctp.receive_data(data, fake_addr);
                    }
                }
            });
        }
        // Either wait for a connection or just initiate one with the same ports
        // let assoc = sctp.accept().await;
        let assoc = sctp.connect(fake_addr, 200, 100).await;
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
            let mut ctr = 1;
            let mut bytes_ctr = 0u64;
            let mut start = std::time::Instant::now();
            loop {
                let data = rx.recv_data(0).await.unwrap();
                bytes_ctr += data.len() as u64;
                ctr += 1;
                if ctr % 10_000 == 0 {
                    eprintln!(
                        "{:12}",
                        (1_000_000 * bytes_ctr)
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

                let mut chunk_buf = BytesMut::with_capacity(PMTU - 100).limit(PMTU - 100);
                tokio::select! {
                    packet = collect_all_chunks(&tx, &mut chunk_buf) => {
                        send_to(&transport_tx, packet.unwrap(), chunk_buf.into_inner().freeze()).await.unwrap();
                    }
                    _ = tokio::time::sleep(timeout) => {
                        tx.handle_timeout(timer);
                    }
                };
            } else {
                let mut chunk_buf = BytesMut::with_capacity(PMTU - 100).limit(PMTU - 100);
                let packet = collect_all_chunks(&tx, &mut chunk_buf).await;

                send_to(&transport_tx, packet.unwrap(), chunk_buf.into_inner().freeze())
                    .await
                    .unwrap();
            }
        }
    });
    runtime
}

async fn collect_all_chunks<FakeContent: FakeAddr>(
    tx: &Arc<AssociationTx<FakeContent>>,
    chunks: &mut impl BufMut,
) -> Result<Packet, ()> {
    let mut byte_ctr = 12;
    let (packet, chunk) = tx.poll_chunk_to_send(chunks.remaining_mut(), 0).await?;
    chunk.serialize(chunks);
    byte_ctr += chunk.serialized_size();
    while let Some((_, chunk)) = tx
        .try_poll_chunk_to_send(chunks.remaining_mut(), byte_ctr)
        .some()
    {
        chunk.serialize(chunks);
    }
    Ok(packet)
}

async fn send_to(
    socket: &Sender<Bytes>,
    packet: Packet,
    chunkbuf: Bytes,
) -> Result<(), tokio::io::Error> {
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    socket.send(buf.freeze()).await.unwrap();
    Ok(())
}

async fn send_chunk<FakeContent: FakeAddr>(
    socket: &Sender<Bytes>,
    packet: Packet,
    chunk: Chunk<FakeContent>,
) -> Result<(), tokio::io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    socket.send(buf.freeze()).await.unwrap();
    Ok(())
}
