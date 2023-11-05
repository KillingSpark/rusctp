use std::sync::mpsc::Receiver;

use bytes::Bytes;

use crate::packet::Chunk;
use crate::{ChunkKind, AssocId};

pub struct Association {
    id: AssocId,
    receiver: Receiver<(AssocId, Chunk)>,
}

impl Association {
    pub(crate) fn new(id: AssocId, receiver: Receiver<(AssocId, Chunk)>) -> Self {
        Self { id, receiver }
    }

    pub fn id(&self) -> AssocId {
        self.id
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
