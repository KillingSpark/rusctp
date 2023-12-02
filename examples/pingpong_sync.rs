use std::{io, net::UdpSocket, sync::Arc};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc_sync::assoc::Sctp,
    packet::{Chunk, Packet},
    FakeAddr, Settings,
};

fn main() {
    let server_addr = "127.0.0.1:1337";
    let client_addr = "127.0.0.1:1338";

    let j1 = std::thread::spawn(move || {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
            pmtu: 1500,
        }));

        let socket = Arc::new(UdpSocket::bind(client_addr).unwrap());
        socket.connect(server_addr).unwrap();
        let socket_send = Arc::clone(&socket);
        let socket_recv = Arc::clone(&socket);
        sctp.register_address(
            rusctp::TransportAddress::Fake(100u64),
            move |packet, chunk| send_to(&socket_send, packet, chunk),
        );

        eprintln!("Client builds conn");
        let sctp_recv = Arc::clone(&sctp);
        std::thread::spawn(move || {
            loop {
                let mut buf = [0u8; 1024 * 100];
                eprintln!("Wait recv");
                let Ok(size) = socket_recv.recv(&mut buf[..]) else {
                    break;
                };
                eprintln!("Recv");
                sctp_recv.receive_data(
                    Bytes::copy_from_slice(&buf[..size]),
                    rusctp::TransportAddress::Fake(100),
                );
            }
            sctp_recv.kill();
        });
        let assoc = sctp
            .connect(rusctp::TransportAddress::Fake(100), 100, 200)
            .split();
        eprintln!("Client has conn");
        let retx = assoc.0.clone();

        assoc
            .0
            .send_data(Bytes::copy_from_slice(&b"Coolio"[..]), 0, 0, false, false)
            .unwrap();

        std::thread::spawn(move || loop {
            let data = assoc.1.recv_data(0).unwrap();
            eprintln!("Received: {data:?}");
            retx.send_data(data, 0, 0, false, false).unwrap();
        });

        loop {
            let (packet, chunk) = assoc.0.poll_chunk_to_send();
            send_to(&socket, packet, chunk).unwrap();
        }
    });

    let j2 = std::thread::spawn(move || {
        let sctp = Arc::new(Sctp::new(Settings {
            cookie_secret: b"oh boy a secret string".to_vec(),
            incoming_streams: 10,
            outgoing_streams: 10,
            in_buffer_limit: 100 * 1024,
            out_buffer_limit: 100 * 1024,
            pmtu: 1500,
        }));

        let socket = Arc::new(UdpSocket::bind(server_addr).unwrap());
        socket.connect(client_addr).unwrap();
        let socket_send = Arc::clone(&socket);
        let socket_recv = Arc::clone(&socket);

        eprintln!("Server builds conn");
        let sctp_recv = Arc::clone(&sctp);
        std::thread::spawn(move || {
            loop {
                let mut buf = [0u8; 1024 * 100];
                eprintln!("Wait recv");
                let Ok(size) = socket_recv.recv(&mut buf[..]) else {
                    break;
                };
                eprintln!("Recv");
                sctp_recv.receive_data(
                    Bytes::copy_from_slice(&buf[..size]),
                    rusctp::TransportAddress::Fake(200u64),
                );
            }
            sctp_recv.kill();
        });
        sctp.register_address(rusctp::TransportAddress::Fake(200), move |packet, chunk| {
            send_to(&socket_send, packet, chunk)
        });
        let assoc = sctp.accept().split();
        eprintln!("Server has conn");
        let retx = assoc.0.clone();

        std::thread::spawn(move || loop {
            let data = assoc.1.recv_data(0).unwrap();
            eprintln!("Received: {data:?}");
            retx.send_data(data, 0, 0, false, false).unwrap();
        });

        loop {
            let (packet, chunk) = assoc.0.poll_chunk_to_send();
            send_to(&socket, packet, chunk).unwrap();
        }
    });

    j1.join().unwrap();
    j2.join().unwrap();
}

fn send_to<FakeContent: FakeAddr>(
    socket: &UdpSocket,
    packet: Packet,
    chunk: Chunk<FakeContent>,
) -> Result<(), io::Error> {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    eprintln!("Send {packet:?} {chunk:?}");
    socket.send(&buf)?;
    Ok(())
}
