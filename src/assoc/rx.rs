use std::collections::VecDeque;

use crate::{data::DataChunk, AssocId, Chunk};

pub struct AssociationRx {
    id: AssocId,
    in_queue: VecDeque<DataChunk>,
}

pub enum RxNotification {
    Chunk(Chunk),
}

impl AssociationRx {
    pub(crate) fn new(id: AssocId) -> Self {
        Self {
            id,
            in_queue: VecDeque::new(),
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn notification(&mut self, notification: RxNotification, now: std::time::Instant) {
        match notification {
            RxNotification::Chunk(chunk) => self.handle_chunk(chunk, now),
        };
    }

    fn handle_chunk(
        &mut self,
        chunk: Chunk,
        _now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                self.in_queue.push_back(data);
            }
            _ => {
                todo!()
            }
        }
        None
    }
}
