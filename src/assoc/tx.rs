use std::{collections::VecDeque, time::Instant};

use bytes::Buf;

use crate::packet::data::DataChunk;
use crate::{AssocId, Chunk, Packet, TransportAddress};

pub struct AssociationTx {
    id: AssocId,
    primary_path: TransportAddress,
    peer_verification_tag: u32,
    local_port: u16,
    peer_port: u16,

    out_queue: VecDeque<DataChunk>,
    send_next: VecDeque<Chunk>,

    timeout: Option<Instant>,
    tsn_counter: u32,
}

pub enum TxNotification {
    Send(Chunk),
    SAck,
    _PrimaryPathChanged(TransportAddress),
}

impl AssociationTx {
    pub(crate) fn new(
        id: AssocId,
        primary_path: TransportAddress,
        peer_verification_tag: u32,
        local_port: u16,
        peer_port: u16,
        init_tsn: u32,
    ) -> Self {
        Self {
            id,
            primary_path,
            peer_verification_tag,
            local_port,
            peer_port,

            out_queue: VecDeque::new(),
            send_next: VecDeque::new(),

            timeout: None,
            tsn_counter: init_tsn,
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
            TxNotification::SAck => {
                // TODO
            }
        }
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
        if self.out_queue.front()?.serialized_size() < limit {
            let mut packet = self.out_queue.pop_front()?;
            packet.tsn = self.tsn_counter;
            self.tsn_counter += 1;
            packet.end = true;
            Some(packet)
        } else {
            let fragment_data_len = limit - 16;
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

            self.tsn_counter += 1;
            full_packet.begin = false;
            full_packet.buf.advance(fragment_data_len);
            Some(fragment)
        }
    }
}
