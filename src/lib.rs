mod packet;
use bytes::{Bytes, Buf};
pub use packet::*;

mod port;
use port::Port;

use std::{
    collections::{BinaryHeap, HashMap},
    sync::mpsc::Sender,
};

type PortId = u16;

struct PerPortInfo {
    sender: Sender<(PortId, Chunk)>,
    verification_tag: u32,
}

pub struct Sctp<PortCb>
where
    PortCb: FnMut(Port),
{
    new_port_cb: PortCb,
    port_channels: HashMap<PortId, PerPortInfo>,
    ports_need_tick: BinaryHeap<PortId>,
}

impl<PortCb> Sctp<PortCb>
where
    PortCb: FnMut(Port),
{
    pub fn new(port_cb: PortCb) -> Self {
        Self {
            new_port_cb: port_cb,
            port_channels: HashMap::new(),
            ports_need_tick: BinaryHeap::new(),
        }
    }

    pub fn receive_data(&mut self, mut data: Bytes) {
        let Some(packet) = Packet::parse(&data) else {
            return;
        };
        data.advance(12);

        if self.handle_init(&packet, &data) {
            return;
        }

        let port_id = packet.to();
        let Some(port_info) = self.port_channels.get(&port_id) else {
            return;
        };
        if port_info.verification_tag != packet.verification_tag() {
            return;
        }

        while !data.is_empty() {
            let (size, chunk) = Chunk::parse(&data);
            data.advance(size);

            match chunk {
                Ok(chunk) => {
                    if let ChunkKind::Init = chunk.kind() {
                        // TODO this is an error, init chunks may only occur as the first and single chunk in a packet
                    } else {
                        if let Err(_err) = port_info.sender.send((packet.to(), chunk)) {
                            // TODO handle err
                            // maybe just drop? This is basically the receive window right?
                        }
                        self.ports_need_tick.push(port_id)
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

    fn handle_init(&mut self, packet: &Packet, data: &Bytes) -> bool {
        let (_size, Ok(chunk)) = Chunk::parse(data) else {
            return false;
        };
        // TODO make sure size and data.len() match
        if let ChunkKind::Init = chunk.into_kind() {
            self.make_new_port(packet);
            true
        } else {
            false
        }
    }

    pub fn ports_need_tick(&self) -> impl Iterator<Item = PortId> + '_ {
        self.ports_need_tick.iter().copied()
    }

    fn make_new_port(&mut self, packet: &Packet) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.port_channels.insert(
            packet.to(),
            PerPortInfo {
                sender: sender,
                verification_tag: packet.verification_tag(),
            },
        );
        (self.new_port_cb)(Port::new(packet.to(), receiver))
    }
}
