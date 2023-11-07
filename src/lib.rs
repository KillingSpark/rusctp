mod packet;
use bytes::{Buf, Bytes, BytesMut};
pub use packet::*;

pub mod assoc;
use assoc::Association;
use packet::{
    cookie::{Cookie, StateCookie},
    init::{InitAck, InitChunk},
};

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

pub enum TxNotification {
    Send(Chunk),
    _PrimaryPathChanged(TransportAddress),
}
pub enum RxNotification {
    Chunk(Chunk),
}
struct PerAssocInfo {
    verification_tag: u32,
}

pub struct Settings {}

pub struct Sctp {
    assoc_id_gen: u64,
    new_assoc: Option<Association>,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocAlias, AssocId>,

    half_open_assocs: HashMap<AssocId, AssocAlias>,

    cookie_secret: Vec<u8>,

    tx_notifications: VecDeque<(AssocId, TxNotification)>,
    send_immediate: VecDeque<(TransportAddress, Bytes)>,
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

            half_open_assocs: HashMap::new(),

            tx_notifications: VecDeque::new(),
            rx_notifications: VecDeque::new(),
            send_immediate: VecDeque::new(),
        }
    }

    pub fn init_association(
        &mut self,
        peer_addr: TransportAddress,
        peer_port: u16,
        local_port: u16,
    ) -> AssocId {
        let init_chunk = Chunk::Init(InitChunk {
            initiate_tag: 0,
            a_rwnd: 1500,
            outbound_streams: 1,
            inbound_streams: 1,
            initial_tsn: 1337,

            aliases: vec![],
            cookie_preservative_msec: None,
            ecn_capable: None,
            supported_addr_types: None,
        });

        let packet = Packet::new(peer_port, local_port, 1337);

        let mut packet_header = BytesMut::with_capacity(12);
        let mut chunks = BytesMut::with_capacity(init_chunk.serialized_size());
        init_chunk.serialize(&mut chunks);
        let chunks = chunks.freeze();
        packet.serialize(&mut packet_header, &chunks);

        let assoc_id = self.next_assoc_id();
        let alias = AssocAlias {
            peer_addr,
            peer_port,
            local_port,
        };
        self.half_open_assocs.insert(assoc_id, alias);
        // TODO do something with chunks and packet_header
        assoc_id
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

        // Either we have accepted a new association here
        let new_assoc_id = self.handle_cookie_echo(&packet, &mut data);

        // Or we get an ack on an association we initiated
        let new_assoc_id = new_assoc_id.or_else(|| self.handle_init_ack(&packet, &mut data, from));

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
        if assoc_info.verification_tag != packet.verification_tag() {
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

    /// Returns true if the packet should not be processed further
    fn handle_init(&mut self, packet: &Packet, data: &Bytes, from: TransportAddress) -> bool {
        if !Chunk::is_init(data) {
            return false;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            // Does not parse correctly.
            // Handling this correctly is done in process_chunks.
            return false;
        };
        if let Chunk::Init(init) = chunk {
            if size != data.len() {
                // This is illegal, the init needs to be the only chunk in the packet
                // -> stop processing this
                return true;
            }
            let mac = Cookie::calc_mac(
                from,
                &init.aliases,
                packet.from(),
                packet.to(),
                &self.cookie_secret,
            );
            let cookie = StateCookie::Ours(Cookie {
                init_address: from,
                aliases: init.aliases,
                peer_port: packet.from(),
                local_port: packet.to(),
                mac,
            });
            let init_ack = Chunk::InitAck(self.create_init_ack(cookie));
            let mut buf = BytesMut::with_capacity(init_ack.serialized_size());
            // TODO put packet header here
            init_ack.serialize(&mut buf);
            self.send_immediate.push_back((from, buf.freeze()));
            // handled the init correctly, no need to process the packet any further
            true
        } else {
            unreachable!("We checked above that this is an init chunk")
        }
    }

    fn create_init_ack(&self, _cookie: StateCookie) -> InitAck {
        unimplemented!()
    }

    fn handle_cookie_echo(&mut self, packet: &Packet, data: &mut Bytes) -> Option<AssocId> {
        if !Chunk::is_cookie_echo(data) {
            return None;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return None;
        };
        let Chunk::StateCookie(mut cookie) = chunk else {
            return None;
        };
        let Some(cookie) = cookie.make_ours() else {
            return None;
        };

        let calced_mac = Cookie::calc_mac(
            cookie.init_address,
            &cookie.aliases,
            cookie.peer_port,
            cookie.local_port,
            &self.cookie_secret,
        );

        if calced_mac != cookie.mac {
            // TODO maybe bail more drastically?
            return None;
        }

        data.advance(size);
        let assoc_id = self.make_new_assoc(packet, cookie.init_address, &cookie.aliases);
        self.tx_notifications
            .push_back((assoc_id, TxNotification::Send(Chunk::StateCookieAck)));
        Some(assoc_id)
    }

    fn handle_init_ack(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress,
    ) -> Option<AssocId> {
        if !Chunk::is_init_ack(data) {
            return None;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return None;
        };
        if let Chunk::InitAck(init_ack) = chunk {
            data.advance(size);
            let assoc_id = self.make_new_assoc(packet, from, &init_ack.aliases);
            // TODO send cookie echo
            Some(assoc_id)
        } else {
            None
        }
    }

    fn make_new_assoc(
        &mut self,
        packet: &Packet,
        init_address: TransportAddress,
        alias_addresses: &[TransportAddress],
    ) -> AssocId {
        let assoc_id = self.next_assoc_id();
        self.assoc_infos.insert(
            assoc_id,
            PerAssocInfo {
                verification_tag: packet.verification_tag(),
            },
        );
        let original_alias = AssocAlias {
            peer_addr: init_address,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        self.aliases.insert(original_alias, assoc_id);
        for alias_addr in alias_addresses {
            let mut alias = original_alias;
            alias.peer_addr = *alias_addr;
            self.aliases.insert(alias, assoc_id);
        }
        self.new_assoc = Some(Association::new(assoc_id, init_address));
        assoc_id
    }

    fn next_assoc_id(&mut self) -> AssocId {
        self.assoc_id_gen += 1;
        AssocId(self.assoc_id_gen)
    }

    pub fn rx_notifications(&mut self) -> impl Iterator<Item = (AssocId, RxNotification)> + '_ {
        self.rx_notifications.drain(..)
    }
    pub fn tx_notifications(&mut self) -> impl Iterator<Item = (AssocId, TxNotification)> + '_ {
        self.tx_notifications.drain(..)
    }
}
