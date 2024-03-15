use std::fmt::Debug;
use std::{collections::VecDeque, time::Instant};

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{HeartBeatAck, Sequence, Tsn};
use crate::{AssocId, Chunk, FakeAddr, Packet, TransportAddress};
use bytes::{Buf, Bytes};

use self::pmtu::PmtuProbe;
use self::timeouts::Timer;

use super::srtt::Srtt;
use super::ShutdownState;

mod ack;
mod congestion;
mod pmtu;
pub mod timeouts;

#[cfg(test)]
mod tests;

pub struct AssociationTx<FakeContent: FakeAddr> {
    id: AssocId,
    primary_path: TransportAddress<FakeContent>,
    primary_congestion: congestion::PerDestinationInfo,

    peer_verification_tag: u32,
    local_port: u16,
    peer_port: u16,

    out_queue: VecDeque<DataChunk>,
    resend_queue: VecDeque<ResendEntry>,
    send_next: VecDeque<Chunk<FakeContent>>,

    tsn_counter: Tsn,
    last_acked_tsn: Tsn,
    peer_last_acked_tsn: Tsn,
    duplicated_acks: usize,

    out_buffer_limit: usize,
    current_out_buffered: usize,
    current_in_flight: usize,
    per_stream: Vec<PerStreamInfo>,

    peer_rcv_window: u32,
    srtt: Srtt,

    timer_ctr: u64,
    rto_timer: Option<Timer>,
    shutdown_rto_timer: Option<Timer>,
    heartbeat_timer: Option<Timer>,
    heartbeats_unacked: usize,
    pmtu_probe: PmtuProbe,

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
pub enum TxNotification<FakeContent: FakeAddr> {
    Send(Chunk<FakeContent>),
    SAck(SelectiveAck, Instant),
    Abort,
    Shutdown,
    PeerShutdown,
    PeerShutdownAck,
    PeerShutdownComplete,
    HeartBeatAck(HeartBeatAck, Instant),
    _PrimaryPathChanged(TransportAddress<FakeContent>),
}

pub struct AssocTxSettings<FakeContent: FakeAddr> {
    pub primary_path: TransportAddress<FakeContent>,
    pub peer_verification_tag: u32,
    pub local_port: u16,
    pub peer_port: u16,
    pub init_tsn: Tsn,
    pub out_streams: u16,
    pub out_buffer_limit: usize,
    pub peer_arwnd: u32,
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

#[derive(Debug)]
pub enum PollSendResult<T> {
    None,
    Some(T),
    Closed,
}

impl<T> PollSendResult<T> {
    pub fn is_some(&self) -> bool {
        matches!(self, Self::Some(_))
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
    pub fn is_err(&self) -> bool {
        matches!(self, Self::Closed)
    }
    pub fn or_else(self, f: impl FnOnce() -> PollSendResult<T>) -> PollSendResult<T> {
        match self {
            Self::Some(_) | Self::Closed => self,
            Self::None => f(),
        }
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> PollSendResult<U> {
        match self {
            Self::None => PollSendResult::None,
            Self::Closed => PollSendResult::Closed,
            Self::Some(t) => PollSendResult::Some(map(t)),
        }
    }

    #[track_caller]
    pub fn unwrap(self) -> T {
        match self {
            Self::Some(t) => t,
            Self::None => panic!("Was none"),
            Self::Closed => panic!("Was closed"),
        }
    }

    #[track_caller]
    pub fn expect(self, message: &str) -> T {
        match self {
            Self::Some(t) => t,
            Self::None => panic!("{message} Was none!"),
            Self::Closed => panic!("{message} Was closed"),
        }
    }

    pub fn some(self) -> Option<T> {
        match self {
            Self::Some(t) => Some(t),
            Self::None => None,
            Self::Closed => None,
        }
    }
}

impl<T> From<Option<T>> for PollSendResult<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(t) => Self::Some(t),
            None => Self::None,
        }
    }
}

impl<FakeContent: FakeAddr> AssociationTx<FakeContent> {
    pub(crate) fn new(id: AssocId, settings: AssocTxSettings<FakeContent>, _now: Instant) -> Self {
        let AssocTxSettings {
            primary_path,
            peer_verification_tag,
            local_port,
            peer_port,
            init_tsn,
            out_streams,
            out_buffer_limit,
            peer_arwnd,
        } = settings;
        Self {
            id,
            primary_path,
            primary_congestion: congestion::PerDestinationInfo::new(1500),

            peer_verification_tag,
            local_port,
            peer_port,
            tsn_counter: init_tsn,
            last_acked_tsn: Tsn(0),
            peer_last_acked_tsn: Tsn(0),
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
            shutdown_rto_timer: None,
            #[cfg(not(test))]
            heartbeat_timer: Some(Timer::new(_now + std::time::Duration::from_millis(10), 0)),
            // No heartbeats for tests.
            #[cfg(test)]
            heartbeat_timer: None,
            heartbeats_unacked: 0,
            pmtu_probe: PmtuProbe::new(),
            timer_ctr: 1,
            shutdown_state: None,
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn notification(
        &mut self,
        notification: TxNotification<FakeContent>,
        _now: std::time::Instant,
    ) {
        match notification {
            TxNotification::Send(Chunk::Data(data)) => self.out_queue.push_back(data),
            TxNotification::Send(chunk) => {
                if let Chunk::SAck(ref sack) = chunk {
                    self.peer_last_acked_tsn = sack.cum_tsn;
                    if let Some(ShutdownState::ShutdownSent) = self.shutdown_state {
                        self.send_next
                            .push_back(Chunk::ShutDown(self.peer_last_acked_tsn))
                    }
                }
                self.send_next.push_back(chunk)
            }
            TxNotification::_PrimaryPathChanged(addr) => self.primary_path = addr,
            TxNotification::SAck(sack, recv_at) => self.handle_sack(sack, recv_at),
            TxNotification::Abort => {
                self.shutdown_state = Some(ShutdownState::AbortReceived);
            }
            TxNotification::Shutdown => {
                if self.shutdown_state.is_none() {
                    self.shutdown_state = Some(ShutdownState::TryingTo);
                }
            }
            TxNotification::PeerShutdown => match self.shutdown_state {
                None => {
                    self.shutdown_state = Some(ShutdownState::ShutdownReceived);
                }
                Some(ShutdownState::ShutdownSent | ShutdownState::ShutdownAckSent) => {
                    self.send_next.push_back(Chunk::ShutDownAck)
                }
                Some(
                    ShutdownState::TryingTo
                    | ShutdownState::ShutdownReceived
                    | ShutdownState::Complete
                    | ShutdownState::AbortReceived,
                ) => {}
            },
            TxNotification::PeerShutdownAck => {
                eprintln!("Got shutdown ack");
                self.send_next
                    .push_back(Chunk::ShutDownComplete { reflected: false });
                self.shutdown_state = Some(ShutdownState::Complete);
            }
            TxNotification::PeerShutdownComplete => {
                self.shutdown_state = Some(ShutdownState::Complete);
            }
            TxNotification::HeartBeatAck(data, now) => self.process_heartbeat_ack(data, now),
        }
    }

    pub fn shutdown_complete(&self) -> bool {
        ShutdownState::is_completely_shutdown(self.shutdown_state.as_ref())
    }

    pub fn initiate_shutdown(&mut self) {
        self.shutdown_state = Some(ShutdownState::TryingTo);
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

    pub fn primary_path(&self) -> TransportAddress<FakeContent> {
        self.primary_path
    }

    // Collect next chunk if it would still fit inside the limit
    pub fn poll_signal_to_send(
        &mut self,
        limit: usize,
        already_packed: usize,
        now: Instant,
    ) -> PollSendResult<Chunk<FakeContent>> {
        let limit = usize::min(
            limit,
            self.pmtu_probe.get_pmtu() - usize::min(self.pmtu_probe.get_pmtu(), already_packed),
        );

        if let Some(front) = self.send_next.front() {
            if front.serialized_size() <= limit {
                self.send_next.pop_front().into()
            } else {
                if let Chunk::HeartBeat(_) = front {
                    if already_packed == 0 {
                        // Only add a heartbeat probe if it woulnd't drop the other chunks in the packet on fail
                        return self.send_next.pop_front().into();
                    }
                }
                PollSendResult::None
            }
        } else {
            match self.shutdown_state {
                Some(ShutdownState::TryingTo) => {
                    if self.resend_queue.is_empty() && self.out_queue.is_empty() {
                        self.shutdown_state = Some(ShutdownState::ShutdownSent);
                        self.set_shutdown_rto_timeout(now);
                        eprintln!("Send shutdown");
                        PollSendResult::Some(Chunk::ShutDown(self.peer_last_acked_tsn))
                    } else {
                        PollSendResult::None
                    }
                }
                Some(ShutdownState::ShutdownReceived) => {
                    if self.resend_queue.is_empty() && self.out_queue.is_empty() {
                        self.shutdown_state = Some(ShutdownState::ShutdownAckSent);
                        self.set_shutdown_rto_timeout(now);
                        PollSendResult::Some(Chunk::ShutDownAck)
                    } else {
                        PollSendResult::None
                    }
                }
                Some(ShutdownState::AbortReceived) => PollSendResult::Closed,
                Some(ShutdownState::Complete) => PollSendResult::Closed,
                Some(ShutdownState::ShutdownSent) => PollSendResult::None,
                Some(ShutdownState::ShutdownAckSent) => PollSendResult::None,
                None => PollSendResult::None,
            }
        }
    }

    // Section 5 recommends handling of this timer
    // https://www.rfc-editor.org/rfc/rfc2988
    pub fn next_timeout(&self) -> Option<Timer> {
        Self::find_next_timer(&[
            self.shutdown_rto_timer,
            self.rto_timer,
            self.heartbeat_timer,
        ])
    }

    fn find_next_timer(timer: &[Option<Timer>]) -> Option<Timer> {
        let mut minimum: Option<Timer> = None;

        for timer in timer {
            if let Some(current) = minimum {
                if let Some(timer) = timer {
                    let min: Timer = Timer::min(current, *timer);
                    minimum = Some(min);
                }
            } else {
                minimum = *timer;
            }
        }

        minimum
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

    // Collect next chunk if it would still fit inside the limit
    pub fn poll_data_to_send(
        &mut self,
        data_limit: usize,
        already_packed: usize,
        now: Instant,
    ) -> PollSendResult<DataChunk> {
        let x = self._poll_data_to_send(data_limit, already_packed, now);
        if x.is_none() {
            //self.print_state();
        }
        x
    }
    fn _poll_data_to_send(
        &mut self,
        data_limit: usize,
        already_packed: usize,
        now: Instant,
    ) -> PollSendResult<DataChunk> {
        // The data chunk header always takes 16 bytes
        if self.pmtu_probe.get_pmtu() <= already_packed {
            return PollSendResult::None;
        }
        let data_limit = usize::min(data_limit, self.pmtu_probe.get_pmtu() - already_packed);
        if data_limit <= 16 {
            return PollSendResult::None;
        }
        let data_limit = data_limit - 16;

        match self.primary_congestion.state() {
            congestion::CongestionState::LossRecovery => {
                if let Some(front) = self.resend_queue.front_mut() {
                    if front.data.buf.len() <= data_limit && front.marked_for_retransmit {
                        let rtx = front.data.clone();
                        front.marked_for_retransmit = false;
                        if self.rto_timer.is_none() {
                            self.set_rto_timeout(now);
                        }
                        eprintln!(
                            "Slow RTX: {:?} {data_limit} {} {already_packed} {}",
                            rtx.tsn,
                            rtx.buf.len(),
                            self.pmtu_probe.get_pmtu()
                        );
                        return PollSendResult::Some(rtx);
                    }
                } else {
                    // TODO this is an internal bug
                    panic!("We are in loss recovery but have no packets in the resend queue?!")
                }
                PollSendResult::None
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
                        return PollSendResult::Some(packet.data.clone());
                    }
                }
                PollSendResult::None
            }
            congestion::CongestionState::CongestionAvoidance
            | congestion::CongestionState::SlowStart => {
                // anything else is subject to the congestion window limits
                let Some(front) = self.out_queue.front() else {
                    return PollSendResult::None;
                };

                if self.current_in_flight > self.primary_congestion.cwnd {
                    //eprintln!("Cwnd window {} > {}", self.current_in_flight, self.primary_congestion.cwnd);
                    return PollSendResult::None;
                }

                // before sending new packets we need to check the peers receive window
                let data_limit = usize::min(data_limit, self.peer_rcv_window as usize);

                let front_buf_len = front.buf.len();
                // check if we can just send the next chunk entirely or if we need to fragment
                let packet = if front_buf_len <= data_limit {
                    let Some(mut packet) = self.out_queue.pop_front() else {
                        return PollSendResult::None;
                    };
                    packet.tsn = self.tsn_counter;
                    packet.end = true;

                    packet
                } else if front_buf_len > self.pmtu_probe.get_pmtu() {
                    let fragment_data_len = data_limit;
                    if fragment_data_len == 0 {
                        return PollSendResult::None;
                    }

                    let Some(full_packet) = self.out_queue.front_mut() else {
                        return PollSendResult::None;
                    };
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
                    return PollSendResult::None;
                };

                // eprintln!("Put new packet on the wire: {:?}", packet.tsn);
                self.tsn_counter = self.tsn_counter.increase();
                self.peer_rcv_window -= packet.buf.len() as u32;
                self.current_in_flight += packet.buf.len();
                self.resend_queue
                    .push_back(ResendEntry::new(packet.clone(), now));
                self.srtt.tsn_sent(packet.tsn, now);
                if self.rto_timer.is_none() {
                    self.set_rto_timeout(now);
                }
                PollSendResult::Some(packet)
            }
        }
    }
}
