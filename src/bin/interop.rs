use std::{
    collections::HashMap,
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

struct Context {
    sctp: Sctp,
    assocs: HashMap<AssocId, Association>,
    addrs: HashMap<TransportAddress, SocketAddr>,
    known_addrs: HashMap<SocketAddr, TransportAddress>,
    socket: UdpSocket,
    current_timeout: Option<Duration>,
    logname: String,
}

fn main() {
    let server_addr = "127.0.0.1:1337";
    let client_addr = "127.0.0.1:1338";
    println!("Listening on addr: {server_addr}");
    println!("Connecting from addr: {client_addr}");

    let jserver = std::thread::spawn(move || {
        let mut server_ctx = Context {
            sctp: Sctp::new(Settings {
                cookie_secret: b"oh boy a secret string".to_vec(),
                incoming_streams: 10,
                outgoing_streams: 10,
                in_buffer_limit: 100 * 1024,
                out_buffer_limit: 100 * 1024,
            }),
            current_timeout: None,
            assocs: HashMap::new(),
            addrs: HashMap::new(),
            known_addrs: HashMap::new(),
            socket: UdpSocket::bind(server_addr).unwrap(),
            logname: "Server: ".to_string(),
        };

        server_ctx.known_addrs.insert(
            SocketAddr::from_str(client_addr).unwrap(),
            TransportAddress::Fake(1),
        );
        server_ctx.addrs.insert(
            TransportAddress::Fake(1),
            SocketAddr::from_str(client_addr).unwrap(),
        );

        server_ctx.run();
    });
    let jclient = std::thread::spawn(move || {
        let mut client_ctx = Context {
            sctp: Sctp::new(Settings {
                cookie_secret: b"oh boy a secret string".to_vec(),
                incoming_streams: 10,
                outgoing_streams: 10,
                in_buffer_limit: 100 * 1024,
                out_buffer_limit: 100 * 1024,
            }),
            current_timeout: None,
            assocs: HashMap::new(),
            addrs: HashMap::new(),
            known_addrs: HashMap::new(),
            socket: UdpSocket::bind(client_addr).unwrap(),
            logname: "Client: ".to_string(),
        };

        client_ctx.known_addrs.insert(
            SocketAddr::from_str(server_addr).unwrap(),
            TransportAddress::Fake(2),
        );
        client_ctx.addrs.insert(
            TransportAddress::Fake(2),
            SocketAddr::from_str(server_addr).unwrap(),
        );
        client_ctx
            .sctp
            .init_association(TransportAddress::Fake(2), 100, 101);
        client_ctx.run();
    });

    jserver.join().unwrap();
    jclient.join().unwrap();
}

impl Context {
    fn run(&mut self) {
        let mut buf = [0u8; 1024 * 8];
        'outer: loop {
            self.handle_notifications();
            self.socket.set_read_timeout(self.current_timeout).unwrap();
            println!("{} Wait for recv", self.logname);
            match self.socket.recv_from(&mut buf) {
                Ok((size, addr)) => self.handle_packet(&buf[..size], addr),
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

    fn handle_notifications(&mut self) {
        eprintln!("{} Handle notifications", self.logname);

        for (to, packet, chunk) in self.sctp.send_immediate() {
            eprintln!("{} Send immediate to: {to:?} {chunk:?}", self.logname);
            if let Some(addr) = self.addrs.get(&to) {
                send_to(&mut self.socket, *addr, packet, chunk);
            }
        }

        for (id, tx_notification) in self.sctp.tx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let tx = assoc.tx_mut();
                tx.notification(tx_notification, std::time::Instant::now());
                let packet = tx.packet_header();
                if let Some(addr) = self.addrs.get(&tx.primary_path()) {
                    while let Some(signal) = tx.poll_signal_to_send(1024) {
                        send_to(&mut self.socket, *addr, packet, signal);
                    }
                }
            }
        }

        for (id, rx_notification) in self.sctp.rx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let (rx, tx) = assoc.split_mut();
                if let Some(addr) = self.addrs.get(&tx.primary_path()) {
                    rx.notification(rx_notification, std::time::Instant::now());
                    let packet = tx.packet_header();
                    if let Some(data) = rx.poll_data(0) {
                        tx.try_send_data(data, 0, 0, false, false);
                        while let Some(data) = tx.poll_data_to_send(1024) {
                            send_to(&mut self.socket, *addr, packet, Chunk::Data(data));
                        }
                    }
                    for tx_notification in rx.tx_notifications() {
                        tx.notification(tx_notification, std::time::Instant::now());
                        while let Some(signal) = tx.poll_signal_to_send(1024) {
                            send_to(&mut self.socket, *addr, packet, signal);
                        }
                        while let Some(data) = tx.poll_data_to_send(1024) {
                            send_to(&mut self.socket, *addr, packet, Chunk::Data(data));
                        }
                    }
                }
            }
        }
    }

    fn handle_packet(&mut self, data: &[u8], from: SocketAddr) {
        eprintln!("{} Handle packet from: {from:?}", self.logname);
        let fake_addr = self.known_addrs.get(&from).copied().unwrap();
        self.sctp
            .receive_data(Bytes::copy_from_slice(data), fake_addr);

        if let Some(mut assoc) = self.sctp.new_assoc() {
            eprintln!("{} got a new association", self.logname);
            assoc.tx_mut().try_send_data(
                Bytes::copy_from_slice(&b"This is cool data"[..]),
                0,
                0,
                false,
                false,
            );
            while let Some(data) = assoc.tx_mut().poll_data_to_send(1024) {
                send_to(
                    &mut self.socket,
                    from,
                    assoc.tx_mut().packet_header(),
                    Chunk::Data(data),
                );
            }
            self.assocs.insert(assoc.id(), assoc);
            self.addrs.insert(fake_addr, from);
        }
    }
}

fn send_to(socket: &mut UdpSocket, from: SocketAddr, packet: Packet, chunk: Chunk) {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    eprintln!("Send to: {from:?} {packet:?} {chunk:?}");
    socket.send_to(&buf, from).unwrap();
}
