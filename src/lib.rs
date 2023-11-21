use bytes::{Buf, Bytes};

pub mod assoc_sync {
    pub mod assoc;
}
pub mod assoc_async {
    pub mod assoc;
}

pub mod assoc;
pub mod packet;
use assoc::{Association, RxNotification, TxNotification};
use packet::{Chunk, Packet, ParseError};

use std::{
    collections::{HashMap, VecDeque},
    net::{Ipv4Addr, Ipv6Addr},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TransportAddress {
    IpV4(Ipv4Addr),
    IpV6(Ipv6Addr),
    Fake(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssocId(u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssocAlias {
    peer_addr: TransportAddress,
    peer_port: u16,
    local_port: u16,
}
struct PerAssocInfo {
    local_verification_tag: u32,
    peer_verification_tag: u32,
}

struct WaitInitAck {
    local_verification_tag: u32,
    local_initial_tsn: u32,
}

struct WaitCookieAck {
    local_verification_tag: u32,
    peer_verification_tag: u32,
    aliases: Vec<TransportAddress>,
    original_address: TransportAddress,
    local_initial_tsn: u32,
    peer_initial_tsn: u32,
    local_in_streams: u16,
    peer_in_streams: u16,
    local_out_streams: u16,
    peer_out_streams: u16,
    peer_arwnd: u32,
}

pub struct Settings {
    pub cookie_secret: Vec<u8>,
    pub incoming_streams: u16,
    pub outgoing_streams: u16,
    pub in_buffer_limit: usize,
    pub out_buffer_limit: usize,
    pub pmtu: usize,
}

pub struct Sctp {
    settings: Settings,

    assoc_id_gen: u64,

    new_assoc: Option<Association>,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocAlias, AssocId>,

    wait_init_ack: HashMap<AssocAlias, WaitInitAck>,
    wait_cookie_ack: HashMap<AssocAlias, WaitCookieAck>,

    tx_notifications: VecDeque<(AssocId, TxNotification)>,
    send_immediate: VecDeque<(TransportAddress, Packet, Chunk)>,
    rx_notifications: VecDeque<(AssocId, RxNotification)>,
}

impl Sctp {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            assoc_id_gen: 1,

            new_assoc: None,
            assoc_infos: HashMap::new(),
            aliases: HashMap::new(),

            wait_init_ack: HashMap::new(),
            wait_cookie_ack: HashMap::new(),

            tx_notifications: VecDeque::new(),
            rx_notifications: VecDeque::new(),
            send_immediate: VecDeque::new(),
        }
    }

    pub fn receive_data(&mut self, mut data: Bytes, from: TransportAddress) {
        let Some(packet) = Packet::parse(&data) else {
            return;
        };
        data.advance(12);

        // If we get an init chunk we only send the init ack and return immediatly
        if self.handle_init(&packet, &data, from) {
            return;
        }

        if self.handle_init_ack(&packet, &mut data, from) {
            return;
        }

        if self.handle_cookie_echo(&packet, &mut data).is_some() {
            return;
        }

        if self.handle_cookie_ack(&packet, &mut data, from).is_some() {
            return;
        }

        // Either we have accepted a new association here
        let new_assoc_id = self.handle_cookie_echo(&packet, &mut data);

        // Or we get an ack on an association we initiated
        let new_assoc_id =
            new_assoc_id.or_else(|| self.handle_cookie_ack(&packet, &mut data, from));

        // Or we need to look the ID up via the aliases
        let assoc_id = new_assoc_id.or_else(|| {
            let alias = AssocAlias {
                peer_addr: from,
                peer_port: packet.from(),
                local_port: packet.to(),
            };
            self.aliases.get(&alias).copied()
        });

        let Some(assoc_id) = assoc_id else {
            return;
        };
        self.process_chunks(assoc_id, from, &packet, data);
    }

    fn process_chunks(
        &mut self,
        assoc_id: AssocId,
        from: TransportAddress,
        packet: &Packet,
        mut data: Bytes,
    ) {
        let Some(assoc_info) = self.assoc_infos.get(&assoc_id) else {
            return;
        };
        if assoc_info.local_verification_tag != packet.verification_tag() {
            return;
        }

        while !data.is_empty() {
            let (size, chunk) = Chunk::parse(&data);
            data.advance(size);

            match chunk {
                Ok(chunk) => {
                    if let Chunk::Init(_) = chunk {
                        // TODO this is an error, init chunks may only occur as the first and single chunk in a packet
                    } else {
                        self.rx_notifications
                            .push_back((assoc_id, RxNotification::Chunk(chunk)));
                    }
                }
                Err(ParseError::Unrecognized { stop, report }) => {
                    if report {
                        self.send_immediate.push_back((
                            from,
                            Packet::new(
                                packet.to(),
                                packet.from(),
                                assoc_info.peer_verification_tag,
                            ),
                            Chunk::OpError,
                        ))
                    }
                    if stop {
                        break;
                    } else {
                        continue;
                    }
                }
                Err(ParseError::Done) => {
                    break;
                }
                Err(ParseError::IllegalFormat) => {
                    break;
                }
            }
        }
    }

    pub fn rx_notifications(&mut self) -> impl Iterator<Item = (AssocId, RxNotification)> + '_ {
        self.rx_notifications.drain(..)
    }
    pub fn tx_notifications(&mut self) -> impl Iterator<Item = (AssocId, TxNotification)> + '_ {
        self.tx_notifications.drain(..)
    }
    pub fn send_immediate(
        &mut self,
    ) -> impl Iterator<Item = (TransportAddress, Packet, Chunk)> + '_ {
        self.send_immediate.drain(..)
    }
    pub fn next_send_immediate(&mut self) -> Option<(TransportAddress, Packet, Chunk)> {
        self.send_immediate.pop_front()
    }
    pub fn has_next_send_immediate(&mut self) -> bool {
        self.send_immediate.front().is_some()
    }
    pub fn new_assoc(&mut self) -> Option<Association> {
        self.new_assoc.take()
    }
}
