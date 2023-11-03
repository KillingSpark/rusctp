use std::sync::mpsc::Receiver;

use crate::packet::Packet;
use crate::PortId;

pub struct Port {
    port_id: PortId,
    receiver: Receiver<Packet>,
}

impl Port {
    pub(crate) fn new(port_id: PortId, receiver: Receiver<Packet>) -> Self {
        Self { port_id, receiver }
    }

    pub fn port_id(&self) -> PortId {
        self.port_id
    }

    pub fn tick(&mut self, _now: std::time::Instant) -> Option<std::time::Instant> {
        let next_tick = None;

        while let Ok(_packet) = self.receiver.try_recv() {
            // TODO handle packet
        }

        next_tick
    }
}
