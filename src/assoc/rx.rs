use std::collections::VecDeque;

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::{AssocId, Chunk};

use super::TxNotification;
pub struct AssociationRx {
    id: AssocId,
    in_queue: VecDeque<DataChunk>,

    tx_notifications: VecDeque<TxNotification>,

    tsn_counter: u32,
}

pub enum RxNotification {
    Chunk(Chunk),
}

impl AssociationRx {
    pub(crate) fn new(id: AssocId, init_tsn: u32) -> Self {
        Self {
            id,
            in_queue: VecDeque::new(),

            tx_notifications: VecDeque::new(),

            tsn_counter: init_tsn - 1,
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

    pub fn tx_notifications(&mut self) -> impl Iterator<Item = TxNotification> + '_ {
        self.tx_notifications.drain(..)
    }

    fn handle_chunk(
        &mut self,
        chunk: Chunk,
        _now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                if data.tsn != self.tsn_counter + 1 {
                    // TODO reorder buffer
                }
                self.tsn_counter += 1;

                self.in_queue.push_back(data);
                self.tx_notifications
                    .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                        cum_tsn: self.tsn_counter,
                        a_rwnd: 1024 * 100, // TODO
                        blocks: vec![],
                        duplicated_tsn: vec![],
                    })))
            }
            Chunk::SAck(_) => self.tx_notifications.push_back(TxNotification::SAck),
            _ => {
                todo!()
            }
        }
        None
    }
}
