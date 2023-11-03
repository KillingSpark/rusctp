mod packet;
pub use packet::*;

mod port;
use port::Port;

use std::{
    collections::{BinaryHeap, HashMap},
    sync::mpsc::Sender,
};

type PortId = u16;

pub struct Sctp<PortCb>
where
    PortCb: FnMut(Port),
{
    new_port_cb: PortCb,
    port_channels: HashMap<PortId, Sender<Packet>>,
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

    pub fn receive_data(&mut self, data: &[u8]) {
        let Some(packet) = Packet::parse(data) else {
            return;
        };
        if let PacketKind::Signal(Signal::Init(port_id)) = packet.packet() {
            self.make_new_port(*port_id);
        } else {
            let port_id = packet.to().port();
            let Some(sender) = self.port_channels.get(&port_id) else {
                return;
            };
            if let Err(_err) = sender.send(packet) {
                // TODO handle err
                // maybe just drop? This is basically the receive window right?
            }
            self.ports_need_tick.push(port_id)
        }
    }

    pub fn ports_need_tick(&self) -> impl Iterator<Item = PortId> + '_ {
        self.ports_need_tick.iter().copied()
    }

    fn make_new_port(&mut self, port_id: PortId) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.port_channels.insert(port_id, sender);
        (self.new_port_cb)(Port::new(port_id, receiver))
    }
}
