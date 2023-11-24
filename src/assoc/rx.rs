use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use bytes::Bytes;

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{Sequence, Tsn};
use crate::{AssocId, Chunk};

use super::TxNotification;
pub struct AssociationRx {
    id: AssocId,

    tx_notifications: VecDeque<TxNotification>,

    tsn_counter: Tsn,

    per_stream: Vec<PerStreamInfo>,
    tsn_reorder_buffer: BTreeMap<Tsn, DataChunk>,

    in_buffer_limit: usize,
    current_in_buffer: usize,
}

struct PerStreamInfo {
    seqnum_ctr: Sequence,
    queue: BTreeMap<Sequence, DataChunk>,
}

pub enum RxNotification {
    Chunk(Chunk),
}

impl AssociationRx {
    pub(crate) fn new(id: AssocId, init_tsn: Tsn, in_streams: u16, in_buffer_limit: usize) -> Self {
        Self {
            id,

            tx_notifications: VecDeque::new(),

            tsn_counter: init_tsn.decrease(),

            per_stream: (0..in_streams)
                .map(|_| PerStreamInfo {
                    seqnum_ctr: Sequence(u16::MAX),
                    queue: BTreeMap::new(),
                })
                .collect(),
            tsn_reorder_buffer: BTreeMap::new(),

            in_buffer_limit,
            current_in_buffer: 0,
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
        now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                self.handle_data_chunk(data);
            }
            Chunk::SAck(sack) => self
                .tx_notifications
                .push_back(TxNotification::SAck((sack, now))),
            _ => {
                todo!()
            }
        }
        None
    }

    pub fn handle_data_chunk(&mut self, data: DataChunk) {
        if let Some(stream_info) = self.per_stream.get_mut(data.stream_id as usize) {
            match data.tsn.cmp(&self.tsn_counter.increase()) {
                Ordering::Greater => {
                    // TSN reordering
                    self.current_in_buffer += data.buf.len();
                    self.tsn_reorder_buffer.insert(data.tsn, data);
                }
                Ordering::Equal => {
                    self.tsn_counter = self.tsn_counter.increase();

                    if self.current_in_buffer + data.buf.len() <= self.in_buffer_limit {
                        // TODO do we ack something even though the stream id was invalid?

                        self.current_in_buffer += data.buf.len();
                        stream_info.queue.insert(data.stream_seq_num, data);

                        self.tx_notifications
                            .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                                cum_tsn: self.tsn_counter,
                                a_rwnd: (self.in_buffer_limit - self.current_in_buffer) as u32,
                                blocks: vec![],
                                duplicated_tsn: vec![],
                            })))
                    }
                }
                Ordering::Less => {
                    // TODO just drop?
                }
            }

            // Check if any reordered packets can now be received
            while self
                .tsn_reorder_buffer
                .first_key_value()
                .map(|(tsn, _)| *tsn == self.tsn_counter.increase())
                .unwrap_or(false)
            {
                let data = self.tsn_reorder_buffer.pop_first().unwrap().1;
                self.handle_data_chunk(data);
            }
        }
    }

    pub fn poll_data(&mut self, stream_id: u16) -> Option<Bytes> {
        if let Some(stream_info) = self.per_stream.get_mut(stream_id as usize) {
            if let Some((seq, _)) = stream_info.queue.first_key_value() {
                if *seq == stream_info.seqnum_ctr.increase() {
                    stream_info.seqnum_ctr = stream_info.seqnum_ctr.increase();
                    let (_, data) = stream_info.queue.pop_first().unwrap();
                    self.current_in_buffer -= data.buf.len();
                    return Some(data.buf);
                }
            }
        }
        None
    }
}
