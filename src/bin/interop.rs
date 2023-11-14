use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    io,
    net::{SocketAddr, UdpSocket},
    str::FromStr,
    time::Duration,
};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc::Association,
    packet::{Chunk, Packet},
    AssocId, Sctp, Settings, TransportAddress,
};

fn main() {
    let mut sctp = Sctp::new(Settings {
        secret: b"oh boy a secret string".to_vec(),
    });

    let addr = "0.0.0.0:1337";
    println!("Listening on addr: {addr}");
    let mut socket = UdpSocket::bind(addr).unwrap();

    let mut buf = [0u8; 1024 * 8];
    let mut current_timeout = None;
    let mut assocs = HashMap::new();
    let mut addrs = HashMap::new();

    addrs.insert(
        TransportAddress::Fake(4),
        SocketAddr::from_str(addr).unwrap(),
    );
    sctp.init_association(TransportAddress::Fake(4), 100, 100);

    'outer: loop {
        handle_notifications(&mut sctp, &mut assocs, &mut addrs, &mut socket);
        socket.set_read_timeout(current_timeout).unwrap();
        println!("Recv");
        match socket.recv_from(&mut buf) {
            Ok((size, addr)) => {
                println!("Recved");
                handle_packet(
                    &buf[..size],
                    addr,
                    &mut current_timeout,
                    &mut sctp,
                    &mut assocs,
                    &mut addrs,
                )
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::TimedOut {
                    continue;
                } else {
                    eprintln!("Error while receiving: {err:?}");
                    break 'outer;
                }
            }
        }
    }
}

fn handle_notifications(
    sctp: &mut Sctp,
    assocs: &mut HashMap<AssocId, Association>,
    addrs: &mut HashMap<TransportAddress, SocketAddr>,
    socket: &mut UdpSocket,
) {
    for (to, packet, chunk) in sctp.send_immediate() {
        if let Some(addr) = addrs.get(&to) {
            send_to(socket, *addr, packet, chunk);
        }
    }

    for (id, tx_notification) in sctp.tx_notifications() {
        if let Some(assoc) = assocs.get_mut(&id) {
            let tx = assoc.tx_mut();
            tx.notification(tx_notification, std::time::Instant::now());
            let packet = tx.packet_header();
            if let Some(addr) = addrs.get(&tx.primary_path()) {
                while let Some(signal) = tx.poll_signal_to_send(1024) {
                    send_to(socket, *addr, packet, signal);
                }
            }
        }
    }

    for (id, rx_notification) in sctp.rx_notifications() {
        if let Some(assoc) = assocs.get_mut(&id) {
            let (rx, tx) = assoc.split_mut();
            rx.notification(rx_notification, std::time::Instant::now());
            for tx_notification in rx.tx_notifications() {
                tx.notification(tx_notification, std::time::Instant::now());
                let packet = tx.packet_header();
                if let Some(addr) = addrs.get(&tx.primary_path()) {
                    while let Some(signal) = tx.poll_signal_to_send(1024) {
                        send_to(socket, *addr, packet, signal);
                    }
                }
            }
        }
    }
}

fn handle_packet(
    data: &[u8],
    addr: SocketAddr,
    current_timeout: &mut Option<Duration>,
    sctp: &mut Sctp,
    assocs: &mut HashMap<AssocId, Association>,
    addrs: &mut HashMap<TransportAddress, SocketAddr>,
) {
    let mut hasher = DefaultHasher::new();
    addr.hash(&mut hasher);
    let fake_addr = TransportAddress::Fake(hasher.finish());
    sctp.receive_data(Bytes::copy_from_slice(data), fake_addr);

    if let Some(assoc) = sctp.new_assoc() {
        assocs.insert(assoc.id(), assoc);
        addrs.insert(fake_addr, addr);
    }
}

fn send_to(socket: &mut UdpSocket, addr: SocketAddr, packet: Packet, chunk: Chunk) {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    eprintln!("Send to: {addr:?} {packet:?} {chunk:?}");
    socket.send_to(&buf, addr).unwrap();
}
