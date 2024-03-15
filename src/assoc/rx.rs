use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{Sequence, Tsn};
use crate::{AssocId, Chunk, FakeAddr};

use super::{ShutdownState, TxNotification};
pub struct AssociationRx<FakeContent: FakeAddr> {
    id: AssocId,

    tx_notifications: VecDeque<TxNotification<FakeContent>>,

    tsn_counter: Tsn,
    last_sent_arwnd: u32,

    full_packets: VecDeque<StreamReceiveEvent>,
    per_stream: Vec<PerStreamInfo>,
    tsn_reorder_buffer: BTreeMap<Tsn, DataChunk>,

    in_buffer_limit: usize,
    current_in_buffer: usize,

    shutdown_state: Option<ShutdownState>,
}

struct PerStreamInfo {
    reassemble_queue: BTreeMap<Sequence, Vec<DataChunk>>,
}

pub enum RxNotification<FakeContent: FakeAddr> {
    Chunk(Chunk<FakeContent>),
}

impl<FakeContent: FakeAddr> RxNotification<FakeContent> {
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
    Data(StreamReceiveEvent),
}

#[derive(Debug)]
pub struct StreamReceiveEvent {
    pub stream: u16,
    pub ppid: u32,
    pub unordered: bool,
    pub data: Vec<DataChunk>,
}

#[derive(Debug)]
pub enum PollDataError {
    Closed,
}

impl<FakeContent: FakeAddr> AssociationRx<FakeContent> {
    pub(crate) fn new(id: AssocId, init_tsn: Tsn, in_streams: u16, in_buffer_limit: usize) -> Self {
        Self {
            id,

            tx_notifications: VecDeque::new(),

            tsn_counter: init_tsn.decrease(),
            last_sent_arwnd: in_buffer_limit as u32,

            full_packets: VecDeque::new(),
            per_stream: (0..in_streams)
                .map(|_| PerStreamInfo {
                    reassemble_queue: BTreeMap::new(),
                })
                .collect(),
            tsn_reorder_buffer: BTreeMap::new(),

            in_buffer_limit,
            current_in_buffer: 0,

            shutdown_state: None,
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn notification(
        &mut self,
        notification: RxNotification<FakeContent>,
        now: std::time::Instant,
    ) {
        match notification {
            RxNotification::Chunk(chunk) => self.handle_chunk(chunk, now),
        };
        self.assert_invariants();
    }

    pub fn tx_notifications(&mut self) -> impl Iterator<Item = TxNotification<FakeContent>> + '_ {
        self.tx_notifications.drain(..)
    }

    fn assert_invariants(&self) {
        // self.print_state();
        let actual_reordered_buffered = self
            .tsn_reorder_buffer
            .iter()
            .map(|x| x.1.buf.len())
            .sum::<usize>();
        let actual_inorder_buffered = self
            .per_stream
            .iter()
            .flat_map(|x| x.reassemble_queue.iter())
            .flat_map(|x| x.1.iter())
            .map(|c| c.buf.len())
            .sum::<usize>()
            + self
                .full_packets
                .iter()
                .flat_map(|x| &x.data)
                .map(|x| x.buf.len())
                .sum::<usize>();
        assert_eq!(
            self.current_in_buffer,
            actual_reordered_buffered + actual_inorder_buffered
        );
    }

    #[allow(dead_code)]
    fn print_state(&self) {
        if self.current_in_buffer == 0 {
            return;
        }
        eprintln!("We have {} bytes buffered", self.current_in_buffer);
        eprintln!("we have {} in tsn reorder", self.tsn_reorder_buffer.len());
        eprintln!("We have {} full packets", self.full_packets.len());
        eprintln!(
            "Gap {:?} -> {:?}",
            self.tsn_counter,
            self.tsn_reorder_buffer.first_key_value()
        );
        self.per_stream.iter().for_each(|stream| {
            eprintln!(
                "    Stream has {} in reassemble queue , {}",
                stream.reassemble_queue.len(),
                stream
                    .reassemble_queue
                    .iter()
                    .flat_map(|x| x.1.iter())
                    .map(|x| x.buf.len())
                    .sum::<usize>()
            )
        });
    }

    fn handle_chunk(
        &mut self,
        chunk: Chunk<FakeContent>,
        now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                self.handle_data_chunk(data);
            }
            Chunk::SAck(sack) => self
                .tx_notifications
                .push_back(TxNotification::SAck(sack, now)),
            Chunk::Abort { .. } => {
                self.shutdown_state = Some(ShutdownState::AbortReceived);
                self.tx_notifications.push_back(TxNotification::Abort)
            }
            Chunk::ShutDown { .. } => {
                self.shutdown_state = Some(ShutdownState::ShutdownReceived);
                self.tx_notifications
                    .push_back(TxNotification::PeerShutdown);
            }
            Chunk::ShutDownAck => {
                self.shutdown_state = Some(ShutdownState::Complete);
                self.tx_notifications
                    .push_back(TxNotification::PeerShutdownAck);
            }
            Chunk::ShutDownComplete { .. } => {
                self.shutdown_state = Some(ShutdownState::Complete);
                self.tx_notifications
                    .push_back(TxNotification::PeerShutdownComplete);
            }
            Chunk::HeartBeat(data) => {
                self.tx_notifications
                    .push_back(TxNotification::Send(Chunk::HeartBeatAck(data.into())));
            }
            Chunk::HeartBeatAck(ack) => {
                self.tx_notifications
                    .push_back(TxNotification::HeartBeatAck(ack, now));
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

        self.tx_notifications
            .push_back(TxNotification::Send(Chunk::SAck(SelectiveAck {
                cum_tsn: self.tsn_counter,
                a_rwnd,
                blocks,
                duplicated_tsn: vec![],
            })));
    }

    fn handle_in_order_packet(&mut self, data: DataChunk) -> bool {
        let stream_id = data.stream_id;
        if let Some(stream_info) = self.per_stream.get_mut(stream_id as usize) {
            if self.current_in_buffer + data.buf.len() <= self.in_buffer_limit {
                self.tsn_counter = self.tsn_counter.increase();
                self.current_in_buffer += data.buf.len();
                let is_end = data.end;
                let seqnum = data.stream_seq_num;
                stream_info
                    .reassemble_queue
                    .entry(data.stream_seq_num)
                    .or_insert_with(Vec::new)
                    .push(data);

                if is_end {
                    let full = stream_info.reassemble_queue.remove(&seqnum).unwrap();
                    self.full_packets.push_back(StreamReceiveEvent {
                        stream: stream_id,
                        ppid: full[0].ppid,
                        unordered: false,
                        data: full,
                    });
                }

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
                if self.tsn_reorder_buffer.contains_key(&data.tsn) {
                    // already know this
                    return;
                }
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

    pub fn poll_data(&mut self) -> PollDataResult {
        if self.shutdown_state.is_some() {
            PollDataResult::Error(PollDataError::Closed)
        } else {
            if let Some(data) = self.full_packets.pop_front() {
                self.current_in_buffer -= data.data.iter().map(|x| x.buf.len()).sum::<usize>();
                return PollDataResult::Data(data);
            }

            PollDataResult::NoneAvailable
        }
    }
}
