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
    last_sent_arwnd: u32,

    per_stream: Vec<PerStreamInfo>,
    tsn_reorder_buffer: BTreeMap<Tsn, DataChunk>,

    in_buffer_limit: usize,
    current_in_buffer: usize,

    closed: bool,
}

struct PerStreamInfo {
    seqnum_ctr: Sequence,
    queue: BTreeMap<Sequence, DataChunk>,
}

pub enum RxNotification {
    Chunk(Chunk),
}

impl RxNotification {
    pub fn get_stream_id(&self) -> Option<u16> {
        if let Self::Chunk(Chunk::Data(data)) = self {
            Some(data.stream_id)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum PollDataResult {
    Error(PollDataError),
    NoneAvailable,
    Data(Bytes),
}

#[derive(Debug)]
pub enum PollDataError {
    Closed,
}

impl AssociationRx {
    pub(crate) fn new(id: AssocId, init_tsn: Tsn, in_streams: u16, in_buffer_limit: usize) -> Self {
        Self {
            id,

            tx_notifications: VecDeque::new(),

            tsn_counter: init_tsn.decrease(),
            last_sent_arwnd: in_buffer_limit as u32,

            per_stream: (0..in_streams)
                .map(|_| PerStreamInfo {
                    seqnum_ctr: Sequence(u16::MAX),
                    queue: BTreeMap::new(),
                })
                .collect(),
            tsn_reorder_buffer: BTreeMap::new(),

            in_buffer_limit,
            current_in_buffer: 0,

            closed: false,
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
            Chunk::Abort => {
                self.closed = true;
                self.tx_notifications.push_back(TxNotification::Abort)
            }
            _ => {
                todo!()
            }
        }
        None
    }

    fn queue_ack(&mut self) {
        let a_rwnd = (self.in_buffer_limit - self.current_in_buffer) as u32;
        self.last_sent_arwnd = a_rwnd;

        let mut blocks = vec![];
        if let Some((key, _tsn)) = self.tsn_reorder_buffer.first_key_value() {
            let mut block_start = key.0;
            let mut block_end = key.0 - 1;
            for reordered in self.tsn_reorder_buffer.iter() {
                if reordered.0 .0 == block_end + 1 {
                    block_end += 1;
                } else {
                    blocks.push((
                        (block_start - self.tsn_counter.0) as u16,
                        (block_end - self.tsn_counter.0) as u16,
                    ));
                    block_start = reordered.0 .0;
                    block_end = reordered.0 .0;
                }
            }
            blocks.push((
                (block_start - self.tsn_counter.0) as u16,
                (block_end - self.tsn_counter.0) as u16,
            ));
            //eprintln!("CumTsn {:?} Gap blocks: {:?}", self.tsn_counter, blocks);
        }

        // TODO send tsn gap blocks
        self.tx_notifications
            .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                cum_tsn: self.tsn_counter,
                a_rwnd,
                blocks,
                duplicated_tsn: vec![],
            })));
    }

    fn handle_in_order_packet(&mut self, data: DataChunk) -> bool {
        if let Some(stream_info) = self.per_stream.get_mut(data.stream_id as usize) {
            if self.current_in_buffer + data.buf.len() <= self.in_buffer_limit {
                self.tsn_counter = self.tsn_counter.increase();
                self.current_in_buffer += data.buf.len();
                stream_info.queue.insert(data.stream_seq_num, data);

                // Check if any reordered packets can now be received
                while self
                    .tsn_reorder_buffer
                    .first_key_value()
                    .map(|(tsn, _)| *tsn == self.tsn_counter.increase())
                    .unwrap_or(false)
                {
                    let data = self.tsn_reorder_buffer.pop_first().unwrap().1;
                    self.current_in_buffer -= data.buf.len();
                    self.handle_in_order_packet(data);
                }
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    pub fn handle_data_chunk(&mut self, data: DataChunk) {
        match data.tsn.cmp(&self.tsn_counter.increase()) {
            Ordering::Greater => {
                // TSN reordering
                // TODO make sure we always have space for packets
                if self.current_in_buffer + data.buf.len() <= self.in_buffer_limit {
                    self.current_in_buffer += data.buf.len();
                    self.tsn_reorder_buffer.insert(data.tsn, data);
                    self.queue_ack();
                    //eprint!("{:?} | ", self.tsn_counter);
                    //self.tsn_reorder_buffer
                    //    .iter()
                    //    .for_each(|x| eprint!("{:?}", x.0));
                    //eprintln!("");
                }
            }
            Ordering::Equal => {
                if self.handle_in_order_packet(data) {
                    self.queue_ack();
                }
            }
            Ordering::Less => {
                // TODO just drop?
            }
        }
    }

    pub fn poll_data(&mut self, stream_id: u16) -> PollDataResult {
        if self.closed {
            PollDataResult::Error(PollDataError::Closed)
        } else {
            if let Some(stream_info) = self.per_stream.get_mut(stream_id as usize) {
                if let Some((seq, _)) = stream_info.queue.first_key_value() {
                    if *seq == stream_info.seqnum_ctr.increase() {
                        stream_info.seqnum_ctr = stream_info.seqnum_ctr.increase();
                        let (_, data) = stream_info.queue.pop_first().unwrap();
                        self.current_in_buffer -= data.buf.len();

                        let a_rwnd = (self.in_buffer_limit - self.current_in_buffer) as u32;

                        if a_rwnd >= self.last_sent_arwnd * 2 {
                            self.last_sent_arwnd = a_rwnd;
                            self.tx_notifications
                                .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                                    cum_tsn: self.tsn_counter,
                                    a_rwnd,
                                    blocks: vec![],
                                    duplicated_tsn: vec![],
                                })));
                        }

                        return PollDataResult::Data(data.buf);
                    }
                }
            }
            PollDataResult::NoneAvailable
        }
    }
}
