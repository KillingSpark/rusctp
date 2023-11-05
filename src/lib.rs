mod packet;
use bytes::{Buf, Bytes, BytesMut};
pub use packet::*;

mod assoc;
use assoc::Association;

use std::{
    collections::{BinaryHeap, HashMap},
    net::{Ipv4Addr, Ipv6Addr},
    sync::mpsc::Sender,
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
    sender: Sender<(AssocId, Chunk)>,
    verification_tag: u32,
}

pub struct Sctp<AssocCb>
where
    AssocCb: FnMut(Association, TransportAddress),
{
    assoc_id_gen: u64,
    new_assoc_cb: AssocCb,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocAlias, AssocId>,
    assocs_need_tick: BinaryHeap<AssocId>,

    cookie_secret: Vec<u8>,
}

impl<AssocCb> Sctp<AssocCb>
where
    AssocCb: FnMut(Association, TransportAddress),
{
    pub fn new(assoc_cb: AssocCb) -> Self {
        Self {
            assoc_id_gen: 1,
            new_assoc_cb: assoc_cb,
            assoc_infos: HashMap::new(),
            aliases: HashMap::new(),
            assocs_need_tick: BinaryHeap::new(),
            cookie_secret: vec![1, 2, 3, 4], // TODO
        }
    }

    pub fn receive_data(
        &mut self,
        mut data: Bytes,
        from: TransportAddress,
        mut send_data: impl FnMut(Bytes, TransportAddress),
    ) {
        let Some(packet) = Packet::parse(&data) else {
            return;
        };
        data.advance(12);

        // If we get an init chunk we only send the init ack and return immediatly
        if self.handle_init(&packet, &data, from, &mut send_data) {
            return;
        }

        // Either we have accepted a new association here
        let new_assoc_id = self.handle_cookie_echo(&packet, &mut data, from, &mut send_data);

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
                        if let Err(_err) = assoc_info.sender.send((assoc_id, chunk)) {
                            // TODO handle err
                            // maybe just drop? This is basically the receive window right?
                        }
                        self.assocs_need_tick.push(assoc_id)
                    }
                }
                Err(UnrecognizedChunkReaction::Skip { report: _ }) => {
                    // TODO report if necessary
                    continue;
                }
                Err(UnrecognizedChunkReaction::Stop { report: _ }) => {
                    // TODO report if necessary
                    break;
                }
            }
        }
    }

    /// Returns true if the packet should not be processed further
    fn handle_init(
        &mut self,
        packet: &Packet,
        data: &Bytes,
        from: TransportAddress,
        mut send_data: impl FnMut(Bytes, TransportAddress),
    ) -> bool {
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
            let mac = StateCookie::calc_mac(
                from,
                &init.aliases,
                packet.from(),
                packet.to(),
                &self.cookie_secret,
            );
            let init_ack = Chunk::InitAck(
                self.create_init_chunk(),
                StateCookie {
                    init_address: from,
                    aliases: init.aliases,
                    peer_port: packet.from(),
                    local_port: packet.to(),
                    mac,
                },
            );
            let mut buf = BytesMut::new();
            // TODO put packet header here
            init_ack.serialize(&mut buf);
            send_data(buf.freeze(), from);
            // handled the init correctly, no need to process the packet any further
            true
        } else {
            unreachable!("We checked above that this is an init chunk")
        }
    }

    fn create_init_chunk(&self) -> InitChunk {
        unimplemented!()
    }

    fn handle_cookie_echo(
        &mut self,
        packet: &Packet,
        data: &mut Bytes,
        from: TransportAddress,
        mut send_data: impl FnMut(Bytes, TransportAddress),
    ) -> Option<AssocId> {
        if !Chunk::is_cookie_echo(data) {
            return None;
        }
        let (size, Ok(chunk)) = Chunk::parse(data) else {
            return None;
        };
        if let Chunk::StateCookie(cookie) = chunk {
            let calced_mac = StateCookie::calc_mac(
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
            let assoc_id = self.make_new_assoc(packet, cookie, from);
            send_data(Chunk::cookie_ack_bytes(), from);
            Some(assoc_id)
        } else {
            None
        }
    }

    fn make_new_assoc(
        &mut self,
        packet: &Packet,
        state_cookie: StateCookie,
        from: TransportAddress,
    ) -> AssocId {
        let (sender, receiver) = std::sync::mpsc::channel();
        let assoc_id = self.next_assoc_id();
        self.assoc_infos.insert(
            assoc_id,
            PerAssocInfo {
                sender,
                verification_tag: packet.verification_tag(),
            },
        );
        let original_alias = AssocAlias {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        self.aliases.insert(original_alias, assoc_id);
        for alias_addr in state_cookie.aliases {
            let mut alias = original_alias;
            alias.peer_addr = alias_addr;
            self.aliases.insert(alias, assoc_id);
        }
        (self.new_assoc_cb)(Association::new(assoc_id, receiver), from);
        assoc_id
    }

    fn next_assoc_id(&mut self) -> AssocId {
        self.assoc_id_gen += 1;
        AssocId(self.assoc_id_gen)
    }

    pub fn assocs_need_tick(&self) -> impl Iterator<Item = AssocId> + '_ {
        self.assocs_need_tick.iter().copied()
    }
}
