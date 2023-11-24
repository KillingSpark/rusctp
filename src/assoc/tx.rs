use std::{collections::VecDeque, time::Instant};

use bytes::{Buf, Bytes};

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{Sequence, Tsn};
use crate::{AssocId, Chunk, Packet, TransportAddress};

use super::srtt::Srtt;

pub struct AssociationTx {
    id: AssocId,
    primary_path: TransportAddress,
    primary_congestion: PerDestinationInfo,

    peer_verification_tag: u32,
    local_port: u16,
    peer_port: u16,

    out_queue: VecDeque<DataChunk>,
    resend_queue: VecDeque<ResendEntry>,
    send_next: VecDeque<Chunk>,

    tsn_counter: Tsn,

    out_buffer_limit: usize,
    current_out_buffered: usize,
    current_in_flight: usize,
    per_stream: Vec<PerStreamInfo>,

    peer_rcv_window: u32,
    srtt: Srtt,
}

struct ResendEntry {
    queued_at: Instant,
    data: DataChunk,
    marked_received: bool,
    marked_for_resend: bool,
}

impl ResendEntry {
    fn new(data: DataChunk, now: Instant) -> Self {
        Self {
            queued_at: now,
            data,
            marked_for_resend: false,
            marked_received: false,
        }
    }
}

#[derive(Clone, Copy)]
struct PerStreamInfo {
    seqnum_ctr: Sequence,
}

enum CongestionState {
    SlowStart,
    CongestionAvoidance,
}

struct PerDestinationInfo {
    state: CongestionState,
    pmcds: usize,
    cwnd: usize,
    ssthresh: usize,
    _partial_bytes_acked: usize,
    bytes_acked_counter: usize,
    bytes_acked_start_tsn: Option<Tsn>,
    last_acked_tsn: Tsn,
    duplicated_acks: usize,
}

impl PerDestinationInfo {
    fn new(pmtu: usize) -> Self {
        let pmcds = pmtu - 12;
        Self {
            state: CongestionState::SlowStart,
            pmcds,
            cwnd: usize::min(4 * pmcds, usize::max(2 * pmcds, 4404)),
            ssthresh: usize::MAX,
            _partial_bytes_acked: 0,
            bytes_acked_counter: 0,
            bytes_acked_start_tsn: None,
            last_acked_tsn: Tsn(0),
            duplicated_acks: 0,
        }
    }

    fn send_limit(&self, current_outstanding: usize) -> usize {
        self.cwnd.saturating_sub(current_outstanding)
    }

    fn bytes_acked(&mut self, bytes_acked: usize, up_to_tsn: Tsn) {
        self.bytes_acked_counter += bytes_acked;
        if let Some(bytes_acked_start_tsn) = { self.bytes_acked_start_tsn } {
            if up_to_tsn >= bytes_acked_start_tsn {
                self.adjust_cwnd();
            }
        }
        if up_to_tsn == self.last_acked_tsn {
            self.duplicated_acks += 1;
            if self.duplicated_acks >= 2 {
                self.ssthresh = self.cwnd / 2;
                self.cwnd = self.ssthresh;
                self.change_state(CongestionState::CongestionAvoidance);
            }
        } else {
            self.last_acked_tsn = up_to_tsn;
            self.duplicated_acks = 0;
        }
    }

    fn rto_expired(&mut self) {
        self.duplicated_acks += 1;
        if self.duplicated_acks >= 2 {
            self.ssthresh = self.cwnd / 2;
            self.cwnd = usize::min(4 * self.pmcds, usize::max(2 * self.pmcds, 4404));
            self.change_state(CongestionState::CongestionAvoidance);
        }
    }

    fn change_state(&mut self, state: CongestionState) {
        self.bytes_acked_start_tsn = None;
        self.bytes_acked_counter = 0;
        self.state = state;
    }

    fn tsn_sent(&mut self, tsn: Tsn) {
        if self.bytes_acked_start_tsn.is_none() {
            self.bytes_acked_start_tsn = Some(tsn);
            self.bytes_acked_counter = 0;
        }
    }

    fn adjust_cwnd(&mut self) {
        let bytes_acked = self.bytes_acked_counter;
        self.bytes_acked_counter = 0;
        self.bytes_acked_start_tsn = None;
        match self.state {
            CongestionState::SlowStart => {
                if bytes_acked >= self.cwnd {
                    self.cwnd += usize::max(bytes_acked, usize::min(self.pmcds, bytes_acked));
                    if self.cwnd >= self.ssthresh {
                        self.cwnd = self.ssthresh;
                        self.state = CongestionState::CongestionAvoidance;
                    }
                }
            }
            CongestionState::CongestionAvoidance => {
                if bytes_acked >= self.cwnd {
                    self.cwnd += usize::min(self.pmcds, bytes_acked);
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum TxNotification {
    Send(Chunk),
    SAck((SelectiveAck, Instant)),
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

pub struct Timer {
    tsn: Tsn,
    at: Instant,
}

impl Timer {
    pub fn at(&self) -> Instant {
        self.at
    }
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
            primary_congestion: PerDestinationInfo::new(pmtu),

            peer_verification_tag,
            local_port,
            peer_port,
            tsn_counter: init_tsn,
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
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    // Section 5 recommends handling of this timer
    // https://www.rfc-editor.org/rfc/rfc2988
    // TODO follow recommendation
    pub fn next_timeout(&self) -> Option<Instant> {
        self.resend_queue
            .front()
            .map(|packet| packet.queued_at + self.srtt.rto_duration())
    }

    pub fn handle_timeout(&mut self, timeout: Timer) {
        if let Some(found) = self
            .resend_queue
            .iter_mut()
            .find(|p| p.data.tsn == timeout.tsn)
        {
            self.primary_congestion.rto_expired();
            self.srtt.rto_expired();
            found.marked_for_resend = true;
        }
    }

    pub fn notification(&mut self, notification: TxNotification, _now: std::time::Instant) {
        match notification {
            TxNotification::Send(Chunk::Data(data)) => self.out_queue.push_back(data),
            TxNotification::Send(chunk) => self.send_next.push_back(chunk),
            TxNotification::_PrimaryPathChanged(addr) => self.primary_path = addr,
            TxNotification::SAck((sack, recv_at)) => self.handle_sack(sack, recv_at),
        }
    }

    pub fn handle_sack(&mut self, sack: SelectiveAck, now: Instant) {
        let mut bytes_acked = 0;
        let mut packet_acked = |acked: &ResendEntry| {
            self.current_out_buffered -= acked.data.buf.len();
            self.current_in_flight -= acked.data.buf.len();
            bytes_acked += acked.data.buf.len();
        };

        while self
            .resend_queue
            .front()
            .map(|packet| packet.data.tsn <= sack.cum_tsn)
            .unwrap_or(false)
        {
            let acked = self.resend_queue.pop_front().unwrap();
            packet_acked(&acked);
        }
        for block in sack.blocks {
            let start = sack.cum_tsn.0 + block.0 as u32;
            let end = sack.cum_tsn.0 + block.1 as u32;

            let range = start..end + 1;
            self.resend_queue.iter_mut().for_each(|packet| {
                if range.contains(&packet.data.tsn.0) {
                    packet.marked_received = true;
                    packet.marked_for_resend = false;
                }
            });
        }
        self.peer_rcv_window = sack.a_rwnd - self.current_in_flight as u32;
        self.primary_congestion
            .bytes_acked(bytes_acked, sack.cum_tsn);
        self.srtt.tsn_acked(sack.cum_tsn, now);
    }

    pub fn try_send_data(
        &mut self,
        data: Bytes,
        stream: u16,
        ppid: u32,
        immediate: bool,
        unordered: bool,
    ) -> Option<Bytes> {
        if self.current_out_buffered + data.len() > self.out_buffer_limit {
            return Some(data);
        }
        let Some(stream_info) = self.per_stream.get_mut(stream as usize) else {
            return Some(data);
        };
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
        None
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
            self.send_next.pop_front()
        } else {
            None
        }
    }

    // Collect next chunk if it would still fit inside the limit
    pub fn poll_data_to_send(&mut self, data_limit: usize, now: Instant) -> Option<DataChunk> {
        // The data chunk header always takes 16 bytes
        let data_limit = data_limit - 16;

        // first send any outstanding packets that have been marked for a resend and fit in the limit
        if let Some(front) = self.resend_queue.front() {
            if front.marked_for_resend {
                if front.data.buf.len() <= data_limit {
                    return Some(front.data.clone());
                } else {
                    return None;
                }
            }
        }

        // anything else is subject to the congestion window limits
        let data_limit = usize::min(
            data_limit,
            self.primary_congestion.send_limit(self.current_in_flight),
        );

        let front = self.out_queue.front()?;
        let front_buf_len = front.buf.len();

        // before sending new packets we need to check the peers receive window
        if usize::min(front_buf_len, data_limit) > self.peer_rcv_window as usize {
            return None;
        }

        // check if we can just send the next chunk entirely or if we need to fragment
        let packet = if front_buf_len < data_limit {
            let mut packet = self.out_queue.pop_front()?;
            packet.tsn = self.tsn_counter;
            packet.end = true;

            packet
        } else if front_buf_len > self.primary_congestion.pmcds {
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
        self.primary_congestion.tsn_sent(packet.tsn);
        self.srtt.tsn_sent(packet.tsn, now);
        Some(packet)
    }
}

#[test]
fn buffer_limits() {
    let mut tx = AssociationTx::new(
        AssocId(0),
        AssocTxSettings {
            primary_path: TransportAddress::Fake(0),
            peer_verification_tag: 1234,
            local_port: 1,
            peer_port: 2,
            init_tsn: Tsn(1),
            out_streams: 1,
            out_buffer_limit: 100,
            peer_arwnd: 10000000,
            pmtu: 10000,
        },
    );
    let send_ten_bytes = |tx: &mut AssociationTx| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    // This should now fail as the buffer is full
    assert!(send_ten_bytes(&mut tx).is_some());

    let packet = tx
        .poll_data_to_send(100, Instant::now())
        .expect("Should return the first packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the second packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the third packet");

    assert_eq!(tx.current_in_flight, 30);
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());

    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: packet.tsn,
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 20);
    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: packet.tsn.increase(),
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 10);
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_some());
}

#[test]
fn arwnd_limits() {
    let mut tx = AssociationTx::new(
        AssocId(0),
        AssocTxSettings {
            primary_path: TransportAddress::Fake(0),
            peer_verification_tag: 1234,
            local_port: 1,
            peer_port: 2,
            init_tsn: Tsn(1),
            out_streams: 1,
            out_buffer_limit: 100,
            peer_arwnd: 20,
            pmtu: 10000,
        },
    );
    let send_ten_bytes = |tx: &mut AssociationTx| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    // prep packets
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: Tsn(100), // Whatever we dont care about out buffer size here
                a_rwnd: 20,
                blocks: vec![],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());
}
