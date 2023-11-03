mod packet;
use packet::*;

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
        let new_port = data[0] == 1; // TODO lel
        if new_port {
            self.make_new_port(0);
        } else {
            let port_id = 0;
            let Some(sender) = self.port_channels.get(&port_id) else {
                return;
            };
            if let Err(_err) = sender.send(Packet::parse(data)) {
                // TODO handle err
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
