mod packet;
use bytes::{Buf, Bytes};

pub mod assoc;
use assoc::{Association, RxNotification, TxNotification};
use packet::{Chunk, Packet, ParseError};

use std::{
    collections::{HashMap, VecDeque},
    net::{Ipv4Addr, Ipv6Addr},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransportAddress {
    IpV4(Ipv4Addr),
    IpV6(Ipv6Addr),
    Fake(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssocId(u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssocAlias {
    peer_addr: TransportAddress,
    peer_port: u16,
    local_port: u16,
}
struct PerAssocInfo {
    local_verification_tag: u32,
    _peer_verification_tag: u32,
}

struct WaitInitAck {
    local_verification_tag: u32,
}

struct WaitCookieAck {
    local_verification_tag: u32,
    peer_verification_tag: u32,
    aliases: Vec<TransportAddress>,
    original_address: TransportAddress,
}

pub struct Settings {}

pub struct Sctp {
    assoc_id_gen: u64,
    new_assoc: Option<Association>,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocAlias, AssocId>,

    wait_init_ack: HashMap<AssocAlias, WaitInitAck>,
    wait_cookie_ack: HashMap<AssocAlias, WaitCookieAck>,

    cookie_secret: Vec<u8>,

    tx_notifications: VecDeque<(AssocId, TxNotification)>,
    send_immediate: VecDeque<(TransportAddress, Packet, Chunk)>,
    rx_notifications: VecDeque<(AssocId, RxNotification)>,
}

impl Sctp {
    pub fn new(_settings: Settings) -> Self {
        Self {
            assoc_id_gen: 1,
            new_assoc: None,
            assoc_infos: HashMap::new(),
            aliases: HashMap::new(),
            cookie_secret: vec![1, 2, 3, 4], // TODO

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
        self.process_chunks(assoc_id, &packet, data);
    }

    fn process_chunks(&mut self, assoc_id: AssocId, packet: &Packet, mut data: Bytes) {
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
                Err(ParseError::Unrecognized { stop, report: _ }) => {
                    // TODO report if necessary
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
}
