use std::collections::{BTreeMap, VecDeque};

use bytes::Bytes;

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::{AssocId, Chunk};

use super::TxNotification;
pub struct AssociationRx {
    id: AssocId,

    tx_notifications: VecDeque<TxNotification>,

    tsn_counter: u32,

    per_stream: Vec<PerStreamInfo>,
    tsn_reorder_buffer: BTreeMap<u32, DataChunk>,

    in_buffer_limit: usize,
    current_in_buffer: usize,
}

struct PerStreamInfo {
    _seqnum_ctr: u16,
    queue: VecDeque<DataChunk>,
}

impl Default for PerStreamInfo {
    fn default() -> Self {
        Self {
            _seqnum_ctr: 0,
            queue: VecDeque::new(),
        }
    }
}

pub enum RxNotification {
    Chunk(Chunk),
}

impl AssociationRx {
    pub(crate) fn new(id: AssocId, init_tsn: u32, in_streams: u16, in_buffer_limit: usize) -> Self {
        Self {
            id,

            tx_notifications: VecDeque::new(),

            tsn_counter: init_tsn - 1,

            per_stream: (0..in_streams).map(|_| PerStreamInfo::default()).collect(),
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
        _now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                self.handle_data_chunk(data);
            }
            Chunk::SAck(sack) => self.tx_notifications.push_back(TxNotification::SAck(sack)),
            _ => {
                todo!()
            }
        }
        None
    }

    pub fn handle_data_chunk(&mut self, data: DataChunk) {
        if let Some(stream_info) = self.per_stream.get_mut(data.stream_id as usize) {
            if data.tsn > self.tsn_counter + 1 {
                eprintln!("REEEEEE ordered: {} {}", self.tsn_counter, data.tsn);
                // TSN reordering
                self.current_in_buffer += data.buf.len();
                self.tsn_reorder_buffer.insert(data.tsn, data);
            } else if data.tsn == self.tsn_counter + 1 {
                self.tsn_counter += 1;

                if self.current_in_buffer + data.buf.len() <= self.in_buffer_limit {
                    // TODO do we ack something even though the stream id was invalid?

                    if stream_info
                        .queue
                        .back()
                        .map(|last| last.stream_seq_num == data.stream_seq_num - 1)
                        .unwrap_or(true)
                    {
                        self.current_in_buffer += data.buf.len();
                        stream_info.queue.push_back(data);

                        self.tx_notifications
                            .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                                cum_tsn: self.tsn_counter,
                                a_rwnd: (self.in_buffer_limit - self.current_in_buffer) as u32,
                                blocks: vec![],
                                duplicated_tsn: vec![],
                            })))
                    } else {
                        // TODO out of order receive
                        eprintln!("Stream seq out of order");
                    }
                }
            } else {
                // TODO just drop?
                eprintln!("In buffer full");
            }

            // Check if any reordered packets can now be received
            while self
                .tsn_reorder_buffer
                .first_key_value()
                .map(|(tsn, _)| *tsn == self.tsn_counter + 1)
                .unwrap_or(false)
            {
                let data = self.tsn_reorder_buffer.pop_first().unwrap().1;
                self.handle_data_chunk(data);
            }
        }
    }

    pub fn poll_data(&mut self, stream_id: u16) -> Option<Bytes> {
        let data = self
            .per_stream
            .get_mut(stream_id as usize)
            .and_then(|stream| stream.queue.pop_front().map(|d| d.buf));
        if let Some(ref data) = data {
            self.current_in_buffer -= data.len();
        }
        data
    }
}
