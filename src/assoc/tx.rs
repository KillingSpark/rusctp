use std::{collections::VecDeque, time::Instant};

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{Sequence, Tsn};
use crate::{AssocId, Chunk, Packet, TransportAddress};
use bytes::{Buf, Bytes};

use super::ShutdownState;
use super::srtt::Srtt;

mod congestion;

#[cfg(test)]
mod tests;

pub struct AssociationTx {
    id: AssocId,
    primary_path: TransportAddress,
    primary_congestion: congestion::PerDestinationInfo,

    peer_verification_tag: u32,
    local_port: u16,
    peer_port: u16,

    out_queue: VecDeque<DataChunk>,
    resend_queue: VecDeque<ResendEntry>,
    send_next: VecDeque<Chunk>,

    tsn_counter: Tsn,
    last_acked_tsn: Tsn,
    duplicated_acks: usize,

    out_buffer_limit: usize,
    current_out_buffered: usize,
    current_in_flight: usize,
    per_stream: Vec<PerStreamInfo>,

    peer_rcv_window: u32,
    srtt: Srtt,

    timer_ctr: u64,
    rto_timer: Option<Timer>,

    shutdown_state: Option<ShutdownState>,
}

struct ResendEntry {
    queued_at: Instant,
    data: DataChunk,
    marked_for_retransmit: bool,
    marked_for_fast_retransmit: bool,
    marked_was_fast_retransmit: bool,
    partially_acked: bool,
}

impl ResendEntry {
    fn new(data: DataChunk, now: Instant) -> Self {
        Self {
            queued_at: now,
            data,
            marked_for_retransmit: false,
            marked_for_fast_retransmit: false,
            marked_was_fast_retransmit: false,
            partially_acked: false,
        }
    }
}

#[derive(Clone, Copy)]
struct PerStreamInfo {
    seqnum_ctr: Sequence,
}

#[derive(Debug)]
pub enum TxNotification {
    Send(Chunk),
    SAck((SelectiveAck, Instant)),
    Abort,
    Shutdown,
    PeerShutdown,
    PeerShutdownAck,
    PeerShutdownComplete,
    _PrimaryPathChanged(TransportAddress),
}

pub struct AssocTxSettings {
    pub primary_path: TransportAddress,
    pub peer_verification_tag: u32,
    pub local_port: u16,
    pub peer_port: u16,
    pub init_tsn: Tsn,
    pub out_streams: u16,
    pub out_buffer_limit: usize,
    pub peer_arwnd: u32,
    pub pmtu: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Timer {
    marker: u64,
    at: Instant,
}

impl Timer {
    pub fn at(&self) -> Instant {
        self.at
    }
}

#[derive(Debug)]
pub enum SendErrorKind {
    Closed,
    BufferFull,
    UnknownStream,
}

#[derive(Debug)]
pub struct SendError {
    pub data: Bytes,
    pub kind: SendErrorKind,
}

impl AssociationTx {
    pub(crate) fn new(id: AssocId, settings: AssocTxSettings) -> Self {
        let AssocTxSettings {
            primary_path,
            peer_verification_tag,
            local_port,
            peer_port,
            init_tsn,
            out_streams,
            out_buffer_limit,
            peer_arwnd,
            pmtu,
        } = settings;
        Self {
            id,
            primary_path,
            primary_congestion: congestion::PerDestinationInfo::new(pmtu),

            peer_verification_tag,
            local_port,
            peer_port,
            tsn_counter: init_tsn,
            last_acked_tsn: Tsn(0),
            duplicated_acks: 0,
            out_buffer_limit,
            peer_rcv_window: peer_arwnd,

            out_queue: VecDeque::new(),
            current_in_flight: 0,
            resend_queue: VecDeque::new(),
            send_next: VecDeque::new(),

            per_stream: vec![
                PerStreamInfo {
                    seqnum_ctr: Sequence(0)
                };
                out_streams as usize
            ],
            current_out_buffered: 0,
            srtt: Srtt::new(),
            rto_timer: None,
            timer_ctr: 0,
            shutdown_state: None,
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn notification(&mut self, notification: TxNotification, _now: std::time::Instant) {
        match notification {
            TxNotification::Send(Chunk::Data(data)) => self.out_queue.push_back(data),
            TxNotification::Send(chunk) => self.send_next.push_back(chunk),
            TxNotification::_PrimaryPathChanged(addr) => self.primary_path = addr,
            TxNotification::SAck((sack, recv_at)) => self.handle_sack(sack, recv_at),
            TxNotification::Abort => {
                self.shutdown_state = Some(ShutdownState::AbortReceived);
            }
            TxNotification::Shutdown => {
                self.shutdown_state = Some(ShutdownState::TryingTo);
            }
            TxNotification::PeerShutdown => {
                self.shutdown_state = Some(ShutdownState::ShutdownReceived);
            }
            TxNotification::PeerShutdownAck => {
                self.send_next.push_back(Chunk::ShutDownComplete);
                self.shutdown_state = Some(ShutdownState::Complete);
            }
            TxNotification::PeerShutdownComplete => {
                self.shutdown_state = Some(ShutdownState::Complete);
            }
        }
    }

    pub fn shutdown_complete(&self) -> bool {
        ShutdownState::is_completely_shutdown(self.shutdown_state.as_ref())
    }

    fn process_sack_gap_blocks(
        &mut self,
        sack: &SelectiveAck,
        fully_acked: usize,
        in_flight_before_sack: usize,
    ) {
        let mut partial_bytes_acked = 0;
        let mut block_iter = sack.blocks.iter().map(|(start, end)| {
            (
                Tsn(sack.cum_tsn.0 + *start as u32),
                Tsn(sack.cum_tsn.0 + *end as u32),
            )
        });
        let mut queue_iter = self.resend_queue.iter_mut();

        let mut next_block = block_iter.next();
        let mut next_packet = queue_iter.next();
        while let Some((block_range, packet)) = next_block.zip(next_packet) {
            if packet.partially_acked {
                next_packet = queue_iter.next();
            } else if packet.data.tsn < block_range.0 {
                // Missing packet
                // TODO we may only mark as many packets as fit in a PMTU and only once when we enter fast recovery
                packet.marked_for_fast_retransmit = true;
                next_packet = queue_iter.next();
            } else if packet.data.tsn >= block_range.0 && packet.data.tsn < block_range.1 {
                // Packet received
                // TODO what about sacks that contain this range multiple times?
                partial_bytes_acked += packet.data.buf.len();
                packet.partially_acked = true;
                next_packet = queue_iter.next();
            } else {
                // take next block
                next_block = block_iter.next();
                next_packet = Some(packet);
            }
        }
        self.primary_congestion.bytes_acked(
            partial_bytes_acked,
            fully_acked,
            in_flight_before_sack,
        );
    }

    fn handle_sack(&mut self, sack: SelectiveAck, now: Instant) {
        let in_flight_before_sack = self.current_in_flight;

        if self.last_acked_tsn > sack.cum_tsn {
            // This is a reordered sack, we can safely ignore this
            return;
        }
        if sack.cum_tsn == self.last_acked_tsn {
            self.peer_rcv_window = sack.a_rwnd - self.current_in_flight as u32;
            self.duplicated_acks += 1;
            if self.duplicated_acks >= 2 {
                self.primary_congestion.enter_fast_recovery();
            }
            self.process_sack_gap_blocks(&sack, 0, in_flight_before_sack);
            return;
        }

        self.last_acked_tsn = sack.cum_tsn;
        self.duplicated_acks = 0;

        let mut bytes_acked = 0;

        while self
            .resend_queue
            .front()
            .map(|packet| packet.data.tsn <= sack.cum_tsn)
            .unwrap_or(false)
        {
            let acked = self.resend_queue.pop_front().unwrap();
            self.current_out_buffered -= acked.data.buf.len();
            self.current_in_flight -= acked.data.buf.len();
            bytes_acked += acked.data.buf.len();
        }

        self.process_sack_gap_blocks(&sack, bytes_acked, in_flight_before_sack);

        self.peer_rcv_window = sack.a_rwnd - self.current_in_flight as u32;
        self.srtt.tsn_acked(sack.cum_tsn, now);
        if bytes_acked > 0 {
            if self.current_in_flight > 0 {
                self.set_timeout(now);
            } else {
                self.rto_timer = None;
            }
        }
    }

    pub fn try_send_data(
        &mut self,
        data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) -> Result<(), SendError> {
        if self.shutdown_state.is_some() {
            return Err(SendError {
                data,
                kind: SendErrorKind::Closed,
            });
        }
        let Some(stream_info) = self.per_stream.get_mut(stream as usize) else {
            return Err(SendError {
                kind: SendErrorKind::UnknownStream,
                data,
            });
        };
        if self.current_out_buffered + data.len() > self.out_buffer_limit {
            self.assert_invariants();
            //self.print_state();
            return Err(SendError {
                kind: SendErrorKind::BufferFull,
                data,
            });
        }
        self.current_out_buffered += data.len();
        self.out_queue.push_back(DataChunk {
            tsn: Tsn(0),
            stream_id: stream,
            stream_seq_num: stream_info.seqnum_ctr,
            ppid,
            buf: data,
            immediate,
            unordered,
            begin: true,
            end: true,
        });
        stream_info.seqnum_ctr = stream_info.seqnum_ctr.increase();
        Ok(())
    }

    pub fn packet_header(&self) -> Packet {
        Packet::new(self.local_port, self.peer_port, self.peer_verification_tag)
    }

    pub fn primary_path(&self) -> TransportAddress {
        self.primary_path
    }

    // Collect next chunk if it would still fit inside the limit
    pub fn poll_signal_to_send(&mut self, limit: usize) -> Option<Chunk> {
        if self.send_next.front()?.serialized_size() < limit {
            // TODO if this is a sack prepend a shutdown
            self.send_next.pop_front()
        } else {
            match self.shutdown_state {
                Some(ShutdownState::TryingTo) => {
                    if self.resend_queue.is_empty() && self.out_queue.is_empty() {
                        // TODO set retransmit timer for this
                        self.shutdown_state = Some(ShutdownState::ShutdownSent);
                        Some(Chunk::ShutDown)
                    } else {
                        None
                    }
                }
                Some(ShutdownState::ShutdownReceived) => {
                    if self.resend_queue.is_empty() && self.out_queue.is_empty() {
                        // TODO set retransmit timer for this
                        self.shutdown_state = Some(ShutdownState::ShutdownAckSent);
                        Some(Chunk::ShutDownAck)
                    } else {
                        None
                    }
                }
                _ => None
            }
        }
    }

    // Section 5 recommends handling of this timer
    // https://www.rfc-editor.org/rfc/rfc2988
    pub fn next_timeout(&self) -> Option<Timer> {
        self.rto_timer
    }

    pub fn handle_timeout(&mut self, timeout: Timer) {
        if self.resend_queue.is_empty() {
            self.rto_timer = None;
            return;
        }
        if let Some(current_timer) = self.rto_timer {
            if current_timer.marker == timeout.marker {
                self.primary_congestion.rto_expired();
                self.srtt.rto_expired();

                self.resend_queue.iter_mut().for_each(|p| {
                    if !p.marked_for_retransmit
                        && p.queued_at + self.srtt.rto_duration() <= timeout.at
                    {
                        p.marked_for_retransmit = true;
                        p.queued_at = timeout.at;
                    }
                });
            }
        }
        // TODO do we trust this or do we take another "now" Instant in the hope that it will be more precise?
        self.set_timeout(timeout.at);
    }

    fn set_timeout(&mut self, now: Instant) {
        let at = now + self.srtt.rto_duration();
        self.rto_timer = Some(Timer {
            at,
            marker: self.timer_ctr,
        });
        self.timer_ctr = self.timer_ctr.wrapping_add(1);
    }

    fn assert_invariants(&self) {
        assert_eq!(
            self.current_out_buffered,
            self.out_queue
                .iter()
                .map(|x| x.buf.len())
                .chain(self.resend_queue.iter().map(|x| x.data.buf.len()))
                .sum::<usize>()
        );
        assert_eq!(
            self.current_in_flight,
            self.resend_queue
                .iter()
                .map(|x| x.data.buf.len())
                .sum::<usize>()
        );
    }

    #[allow(dead_code)]
    fn print_state(&self) {
        if self.out_queue.is_empty() {
            eprintln!(
                "No send: resend_queue: {:2}, in_flight: {:6}, out_queue: {:4}, out_buffered: {:8}, peer_rcv_wnd: {:8}, free_rcv_wnd: {:8}, cwnd: {:8} cng_state: {:?}",
                self.resend_queue.len(),
                self.current_in_flight,
                self.out_queue.len(),
                self.current_out_buffered,
                self.peer_rcv_window,
                self.peer_rcv_window - self.current_in_flight as u32,
                self.primary_congestion.cwnd,
                self.primary_congestion.state
            );
        }
    }

    // Collect next chunk if it would still fit inside the limit
    pub fn poll_data_to_send(&mut self, data_limit: usize, now: Instant) -> Option<DataChunk> {
        let x = self._poll_data_to_send(data_limit, now);
        if x.is_none() {
            self.assert_invariants();
            //self.print_state();
        }
        x
    }
    fn _poll_data_to_send(&mut self, data_limit: usize, now: Instant) -> Option<DataChunk> {
        // The data chunk header always takes 16 bytes
        let data_limit = data_limit - 16;

        match self.primary_congestion.state() {
            congestion::CongestionState::LossRecovery => {
                if let Some(front) = self.resend_queue.front_mut() {
                    if front.data.buf.len() <= data_limit && front.marked_for_retransmit {
                        let rtx = front.data.clone();
                        front.marked_for_retransmit = false;
                        if self.rto_timer.is_none() {
                            self.set_timeout(now);
                        }
                        //eprintln!("Slow RTX: {:?}", rtx.tsn);
                        return Some(rtx);
                    }
                } else {
                    // TODO this is an internal bug
                    panic!("We are in loss recovery but have no packets in the resend queue?!")
                }
                None
            }
            congestion::CongestionState::FastRecovery => {
                for packet in self.resend_queue.iter_mut() {
                    if packet.marked_for_fast_retransmit
                        && !packet.marked_was_fast_retransmit
                        && packet.data.buf.len() <= data_limit
                    {
                        packet.marked_was_fast_retransmit = true;
                        packet.marked_for_fast_retransmit = false;
                        //eprintln!("Fast RTX {:?}", packet.data.tsn);
                        return Some(packet.data.clone());
                    }
                }
                None
            }
            congestion::CongestionState::CongestionAvoidance
            | congestion::CongestionState::SlowStart => {
                // anything else is subject to the congestion window limits
                let front = self.out_queue.front()?;
                let front_buf_len = front.buf.len();

                // before sending new packets we need to check the peers receive window
                if usize::min(front_buf_len, data_limit) > self.peer_rcv_window as usize {
                    return None;
                }

                // check if we can just send the next chunk entirely or if we need to fragment
                let packet = if front_buf_len < data_limit
                    && self.current_in_flight < self.primary_congestion.cwnd
                {
                    let mut packet = self.out_queue.pop_front()?;
                    packet.tsn = self.tsn_counter;
                    packet.end = true;

                    packet
                } else if false {
                    // TODO I am sure there are better metrics to determin usefulness of fragmentation
                    let fragment_data_len = data_limit;
                    if fragment_data_len == 0 {
                        return None;
                    }

                    let full_packet = self.out_queue.front_mut()?;
                    let fragment = DataChunk {
                        tsn: self.tsn_counter,
                        stream_id: full_packet.stream_id,
                        stream_seq_num: full_packet.stream_seq_num,
                        ppid: full_packet.ppid,
                        buf: full_packet.buf.slice(0..fragment_data_len),

                        immediate: full_packet.immediate,
                        unordered: full_packet.unordered,
                        begin: full_packet.begin,
                        end: false,
                    };

                    full_packet.begin = false;
                    full_packet.buf.advance(fragment_data_len);
                    fragment
                } else {
                    return None;
                };
                self.tsn_counter = self.tsn_counter.increase();
                self.peer_rcv_window -= packet.buf.len() as u32;
                self.current_in_flight += packet.buf.len();
                self.resend_queue
                    .push_back(ResendEntry::new(packet.clone(), now));
                self.srtt.tsn_sent(packet.tsn, now);
                if self.rto_timer.is_none() {
                    self.set_timeout(now);
                }
                Some(packet)
            }
        }
    }
}
