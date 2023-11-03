use std::sync::mpsc::Receiver;

use bytes::Bytes;

use crate::packet::Chunk;
use crate::{ChunkKind, PortId};

pub struct Port {
    port_id: PortId,
    receiver: Receiver<(PortId, Chunk)>,
}

impl Port {
    pub(crate) fn new(port_id: PortId, receiver: Receiver<(PortId, Chunk)>) -> Self {
        Self { port_id, receiver }
    }

    pub fn port_id(&self) -> PortId {
        self.port_id
    }

    pub fn tick(
        &mut self,
        _now: std::time::Instant,
        mut data_cb: impl FnMut(Bytes),
    ) -> Option<std::time::Instant> {
        let next_tick = None;

        while let Ok((_port, chunk)) = self.receiver.try_recv() {
            // TODO handle packet
            match chunk.into_kind() {
                ChunkKind::Data(data) => {
                    data_cb(data.buf);
                }
                _ => {
                    todo!()
                }
            }
        }

        next_tick
    }
}
