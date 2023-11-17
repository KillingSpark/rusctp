use std::{collections::VecDeque, time::Instant};

use bytes::{Buf, Bytes};

use crate::packet::data::DataChunk;
use crate::packet::sack::SelectiveAck;
use crate::packet::{Sequence, Tsn};
use crate::{AssocId, Chunk, Packet, TransportAddress};

pub struct AssociationTx {
    id: AssocId,
    primary_path: TransportAddress,
    primary_congestion: PerDestinationInfo,

    peer_verification_tag: u32,
    local_port: u16,
    peer_port: u16,

    out_queue: VecDeque<DataChunk>,
    resend_queue: VecDeque<DataChunk>,
    send_next: VecDeque<Chunk>,

    timeout: Option<Instant>,
    tsn_counter: Tsn,

    out_buffer_limit: usize,
    current_out_buffered: usize,
    current_in_flight: usize,
    per_stream: Vec<PerStreamInfo>,

    peer_rcv_window: u32,
}

#[derive(Clone, Copy)]
struct PerStreamInfo {
    seqnum_ctr: Sequence,
}

enum CongestionState {
    SlowStart,
    _FastRecover,
    _CongestionAvoidance,
}

struct PerDestinationInfo {
    _state: CongestionState,
    _pcmds: usize,
    cwnd: usize,
    _ssthresh: usize,
    _partial_bytes_acked: usize,
}

impl PerDestinationInfo {
    fn new(pmtu: usize) -> Self {
        let pmcds = pmtu - 12;
        Self {
            _state: CongestionState::SlowStart,
            _pcmds: pmcds,
            cwnd: usize::min(4 * pmcds, usize::max(2 * pmcds, 4404)),
            _ssthresh: usize::MAX,
            _partial_bytes_acked: 0,
        }
    }

    fn send_limit(&self, current_outstanding: usize) -> usize {
        self.cwnd.saturating_sub(current_outstanding)
    }
}

pub enum TxNotification {
    Send(Chunk),
    SAck(SelectiveAck),
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

            timeout: None,

            per_stream: vec![
                PerStreamInfo {
                    seqnum_ctr: Sequence(0)
                };
                out_streams as usize
            ],
            current_out_buffered: 0,
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn tick(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        let mut next_tick = None;

        if let Some(timeout) = self.timeout {
            if timeout > now {
                next_tick = Some(timeout);
            } else {
                self.handle_timeout();
            }
        }

        next_tick
    }

    fn handle_timeout(&mut self) {
        // TODO
    }

    pub fn notification(&mut self, notification: TxNotification, _now: std::time::Instant) {
        match notification {
            TxNotification::Send(Chunk::Data(data)) => self.out_queue.push_back(data),
            TxNotification::Send(chunk) => self.send_next.push_back(chunk),
            TxNotification::_PrimaryPathChanged(addr) => self.primary_path = addr,
            TxNotification::SAck(sack) => {
                let mut packet_acked = |acked: &DataChunk| {
                    self.current_out_buffered -= acked.buf.len();
                    self.current_in_flight -= acked.buf.len();
                };

                while self
                    .resend_queue
                    .front()
                    .map(|packet| packet.tsn <= Tsn(sack.cum_tsn))
                    .unwrap_or(false)
                {
                    let acked = self.resend_queue.pop_front().unwrap();
                    packet_acked(&acked);
                }
                for block in sack.blocks {
                    let start = sack.cum_tsn + block.0 as u32;
                    let end = sack.cum_tsn + block.1 as u32;

                    let range = start..end + 1;
                    self.resend_queue.retain(|packet| {
                        if range.contains(&packet.tsn.0) {
                            packet_acked(packet);
                            false
                        } else {
                            true
                        }
                    });
                }
                self.peer_rcv_window = sack.a_rwnd - self.current_in_flight as u32;
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
    pub fn poll_data_to_send(&mut self, limit: usize) -> Option<DataChunk> {
        let front = self.out_queue.front()?;
        let front_buf_len = front.buf.len();
        let data_limit = usize::min(
            limit - 16,
            self.primary_congestion.send_limit(self.current_in_flight),
        );
        if usize::min(front_buf_len, data_limit) > self.peer_rcv_window as usize {
            return None;
        }

        let packet = if front_buf_len < data_limit {
            let mut packet = self.out_queue.pop_front()?;
            packet.tsn = self.tsn_counter;
            packet.end = true;

            packet
        } else {
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
        };
        self.tsn_counter = self.tsn_counter.increase();
        self.peer_rcv_window -= packet.buf.len() as u32;
        self.current_in_flight += packet.buf.len();
        self.resend_queue.push_back(packet.clone());
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
        .poll_data_to_send(100)
        .expect("Should return the first packet");
    tx.poll_data_to_send(100)
        .expect("Should return the second packet");
    tx.poll_data_to_send(100)
        .expect("Should return the third packet");

    assert_eq!(tx.current_in_flight, 30);
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());

    tx.notification(
        TxNotification::SAck(SelectiveAck {
            cum_tsn: packet.tsn.0,
            a_rwnd: 1000000, // Whatever we just want to have a big receive window here
            blocks: vec![(2, 2)],
            duplicated_tsn: vec![],
        }),
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

    assert!(tx.poll_data_to_send(100).is_some());
    assert!(tx.poll_data_to_send(100).is_some());
    assert!(tx.poll_data_to_send(100).is_none());

    tx.notification(
        TxNotification::SAck(SelectiveAck {
            cum_tsn: 100, // Whatever we dont care about out buffer size here
            a_rwnd: 20,
            blocks: vec![],
            duplicated_tsn: vec![],
        }),
        std::time::Instant::now(),
    );

    assert!(tx.poll_data_to_send(100).is_some());
    assert!(tx.poll_data_to_send(100).is_some());
    assert!(tx.poll_data_to_send(100).is_none());
}
