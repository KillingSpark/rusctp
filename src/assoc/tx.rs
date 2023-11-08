use std::{collections::VecDeque, time::Instant};

use bytes::Bytes;

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
}

pub enum TxNotification {
    Send(Chunk),
    _PrimaryPathChanged(TransportAddress),
}

impl AssociationTx {
    pub(crate) fn new(
        id: AssocId,
        primary_path: TransportAddress,
        peer_verification_tag: u32,
        local_port: u16,
        peer_port: u16,
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
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn packet_header(&self) -> Packet {
        Packet::new(self.local_port, self.peer_port, self.peer_verification_tag)
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
        }
    }

    pub fn poll_data_to_send(&mut self) -> Option<(TransportAddress, Bytes)> {
        // TODO build a packet with all send_next and maybe some data from the out_queue
        _ = self.send_next.front();
        _ = self.out_queue.front();
        None
    }
}
