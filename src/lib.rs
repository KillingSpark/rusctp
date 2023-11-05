mod packet;
use bytes::{Buf, Bytes};
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
pub struct AssocId {
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
    new_assoc_cb: AssocCb,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocId, AssocId>,
    assocs_need_tick: BinaryHeap<AssocId>,
}

impl<AssocCb> Sctp<AssocCb>
where
    AssocCb: FnMut(Association, TransportAddress),
{
    pub fn new(assoc_cb: AssocCb) -> Self {
        Self {
            new_assoc_cb: assoc_cb,
            assoc_infos: HashMap::new(),
            aliases: HashMap::new(),
            assocs_need_tick: BinaryHeap::new(),
        }
    }

    pub fn receive_data(&mut self, mut data: Bytes, from: TransportAddress) {
        let Some(packet) = Packet::parse(&data) else {
            return;
        };
        data.advance(12);

        if self.handle_init(&packet, &data, from) {
            return;
        }

        let assoc_id = AssocId {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        // Either we find the association by this ID or we need to take an indirection through the alias table
        let Some(assoc_info) = self.assoc_infos.get(&assoc_id).or_else(|| {
            self.aliases
                .get(&assoc_id)
                .and_then(|dealiased_id| self.assoc_infos.get(dealiased_id))
        }) else {
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
                    if let ChunkKind::Init(_) = chunk.kind() {
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

    fn handle_init(&mut self, packet: &Packet, data: &Bytes, from: TransportAddress) -> bool {
        let (_size, Ok(chunk)) = Chunk::parse(data) else {
            return false;
        };
        // TODO make sure size and data.len() match
        if let ChunkKind::Init(addrs) = chunk.into_kind() {
            self.make_new_assoc(packet, addrs, from);
            true
        } else {
            false
        }
    }

    pub fn assocs_need_tick(&self) -> impl Iterator<Item = AssocId> + '_ {
        self.assocs_need_tick.iter().copied()
    }

    fn make_new_assoc(&mut self, packet: &Packet, init: InitChunk, from: TransportAddress) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let assoc_id = AssocId {
            peer_addr: from,
            peer_port: packet.from(),
            local_port: packet.to(),
        };
        self.assoc_infos.insert(
            assoc_id,
            PerAssocInfo {
                sender: sender,
                verification_tag: packet.verification_tag(),
            },
        );
        for alias in init.aliases {
            let mut alias_id = assoc_id;
            alias_id.peer_addr = alias;
            self.aliases.insert(alias_id, assoc_id);
        }
        (self.new_assoc_cb)(Association::new(assoc_id, receiver), from)
    }
}
