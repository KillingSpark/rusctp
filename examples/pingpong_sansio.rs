use std::{
    collections::{BinaryHeap, HashMap},
    io,
    net::{SocketAddr, UdpSocket},
    str::FromStr,
    time::Instant,
};

use bytes::{BufMut, Bytes, BytesMut};
use rusctp::{
    assoc::{timeouts::Timer, Association, AssociationTx, PollDataResult, PollSendResult},
    packet::{Chunk, Packet},
    AssocId, FakeAddr, Sctp, Settings, TransportAddress,
};

struct Context<FakeContent: FakeAddr> {
    sctp: Sctp<FakeContent>,
    assocs: HashMap<AssocId, Association<FakeContent>>,
    addrs: HashMap<TransportAddress<FakeContent>, SocketAddr>,
    known_addrs: HashMap<SocketAddr, TransportAddress<FakeContent>>,
    socket: UdpSocket,
    timeouts: BinaryHeap<Timeout>,
    logname: String,
}

struct Timeout {
    assoc: AssocId,
    timer: Timer,
}

impl Eq for Timeout {}

impl PartialEq for Timeout {
    fn eq(&self, other: &Self) -> bool {
        self.timer.at().eq(&other.timer.at())
    }
}

impl Ord for Timeout {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timer.at().cmp(&other.timer.at())
    }
}

impl PartialOrd for Timeout {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.timer.at().partial_cmp(&other.timer.at())
    }
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
            timeouts: BinaryHeap::new(),
            assocs: HashMap::new(),
            addrs: HashMap::new(),
            known_addrs: HashMap::new(),
            socket: UdpSocket::bind(server_addr).unwrap(),
            logname: "Server: ".to_string(),
        };

        server_ctx.known_addrs.insert(
            SocketAddr::from_str(client_addr).unwrap(),
            TransportAddress::Fake(1u64),
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
            timeouts: BinaryHeap::new(),
            assocs: HashMap::new(),
            addrs: HashMap::new(),
            known_addrs: HashMap::new(),
            socket: UdpSocket::bind(client_addr).unwrap(),
            logname: "Client: ".to_string(),
        };

        client_ctx.known_addrs.insert(
            SocketAddr::from_str(server_addr).unwrap(),
            TransportAddress::Fake(2u64),
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

impl<FakeContent: FakeAddr> Context<FakeContent> {
    fn run(&mut self) {
        let mut buf = [0u8; 1024 * 8];
        'outer: loop {
            self.handle_notifications();
            self.socket
                .set_read_timeout(
                    self.timeouts
                        .peek()
                        .map(|timeout| timeout.timer.at() - Instant::now()),
                )
                .unwrap();
            println!("{} Wait for recv", self.logname);
            match self.socket.recv_from(&mut buf) {
                Ok((size, addr)) => self.handle_packet(&buf[..size], addr),
                Err(err) => {
                    if err.kind() == io::ErrorKind::TimedOut {
                        if self
                            .timeouts
                            .peek()
                            .map(|timeout| Instant::now() > timeout.timer.at())
                            .unwrap_or(false)
                        {
                            let timeout = self.timeouts.pop().unwrap();
                            if let Some(assoc) = self.assocs.get_mut(&timeout.assoc) {
                                assoc.tx_mut().handle_timeout(timeout.timer)
                            }
                        }
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
                if let Some(addr) = self.addrs.get(&tx.primary_path()) {
                    Self::send_everything(tx, *addr, &mut self.socket);
                    if let Some(timeout) = tx.next_timeout() {
                        self.timeouts.push(Timeout {
                            assoc: id,
                            timer: timeout,
                        });
                    }
                }
            }
        }

        for (id, rx_notification) in self.sctp.rx_notifications() {
            if let Some(assoc) = self.assocs.get_mut(&id) {
                let (rx, tx) = assoc.split_mut();
                if let Some(addr) = self.addrs.get(&tx.primary_path()) {
                    rx.notification(rx_notification, std::time::Instant::now());

                    // Echo back data we receive
                    match rx.poll_data(0) {
                        PollDataResult::Data(data) => {
                            tx.try_send_data(data, 0, 0, false, false).unwrap();
                        }
                        PollDataResult::NoneAvailable => { /* ignore */ }
                        PollDataResult::Error(err) => panic!("{:?}", err),
                    }

                    for tx_notification in rx.tx_notifications() {
                        tx.notification(tx_notification, std::time::Instant::now());
                    }
                    Self::send_everything(tx, *addr, &mut self.socket);
                    if let Some(timeout) = tx.next_timeout() {
                        self.timeouts.push(Timeout {
                            assoc: id,
                            timer: timeout,
                        });
                    }
                }
            }
        }
    }

    fn send_everything(
        tx: &mut AssociationTx<FakeContent>,
        addr: SocketAddr,
        socket: &mut UdpSocket,
    ) {
        let packet = tx.packet_header();
        while let PollSendResult::Some(signal) = tx.poll_signal_to_send(1024, Instant::now()) {
            send_to(socket, addr, packet, signal);
        }
        while let PollSendResult::Some(data) = tx.poll_data_to_send(1024, Instant::now()) {
            send_to(socket, addr, packet, Chunk::<FakeContent>::Data(data));
        }
    }

    fn handle_packet(&mut self, data: &[u8], from: SocketAddr) {
        eprintln!("{} Handle packet from: {from:?}", self.logname);
        let fake_addr = self.known_addrs.get(&from).copied().unwrap();
        self.sctp
            .receive_data(Bytes::copy_from_slice(data), fake_addr, Instant::now());

        if let Some(mut assoc) = self.sctp.new_assoc() {
            eprintln!("{} got a new association", self.logname);
            assoc
                .tx_mut()
                .try_send_data(
                    Bytes::copy_from_slice(&b"This is cool data"[..]),
                    0,
                    0,
                    false,
                    false,
                )
                .unwrap();
            self.assocs.insert(assoc.id(), assoc);
            self.addrs.insert(fake_addr, from);
        }
    }
}

fn send_to<FakeContent: FakeAddr>(
    socket: &mut UdpSocket,
    from: SocketAddr,
    packet: Packet,
    chunk: Chunk<FakeContent>,
) {
    let mut chunkbuf = BytesMut::new();
    chunk.serialize(&mut chunkbuf);
    let chunkbuf = chunkbuf.freeze();
    let mut buf = BytesMut::new();
    packet.serialize(&mut buf, chunkbuf.clone());
    buf.put_slice(&chunkbuf);

    eprintln!("Send to: {from:?} {packet:?} {chunk:?}");
    socket.send_to(&buf, from).unwrap();
}
