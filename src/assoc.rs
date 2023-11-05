use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

use bytes::{BufMut, Bytes, BytesMut};

use crate::data::DataChunk;
use crate::packet::Chunk;
use crate::{AssocId, SendToAssoc, TransportAddress};

pub struct Association {
    id: AssocId,
    receiver: Receiver<SendToAssoc>,
    primary_path: TransportAddress,
    in_queue: VecDeque<DataChunk>,
    out_queue: VecDeque<DataChunk>,
}

impl Association {
    pub(crate) fn new(
        id: AssocId,
        receiver: Receiver<SendToAssoc>,
        primary_path: TransportAddress,
    ) -> Self {
        Self {
            id,
            receiver,
            primary_path,
            in_queue: VecDeque::new(),
            out_queue: VecDeque::new(),
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn tick(
        &mut self,
        now: std::time::Instant,
        mut data_cb: impl FnMut(Bytes, TransportAddress),
    ) -> Option<std::time::Instant> {
        let mut next_tick = None;

        while let Ok(update) = self.receiver.try_recv() {
            match update {
                SendToAssoc::_PrimaryPathChanged(addr) => self.primary_path = addr,
                SendToAssoc::Chunk(chunk) => {
                    let chunk_tick = self.handle_chunk(chunk, now);
                    next_tick = merge_ticks(next_tick, chunk_tick);
                }
            }
        }

        self.check_out_queue(&mut data_cb);

        next_tick
    }

    fn handle_chunk(
        &mut self,
        chunk: Chunk,
        _now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        match chunk {
            Chunk::Data(data) => {
                self.in_queue.push_back(data);
            }
            _ => {
                todo!()
            }
        }
        None
    }

    fn check_out_queue(&mut self, data_cb: &mut impl FnMut(Bytes, TransportAddress)) {
        let data_bytes = self
            .out_queue
            .iter()
            .map(|buf| buf.buf.len())
            .sum::<usize>();

        let mut buf = BytesMut::with_capacity(data_bytes);
        for data in self.out_queue.drain(..) {
            buf.put_slice(&data.buf);
        }
        data_cb(buf.freeze(), self.primary_path);
    }
}

fn merge_ticks(
    current: Option<std::time::Instant>,
    new: Option<std::time::Instant>,
) -> Option<std::time::Instant> {
    let Some(current) = current else {
        return new;
    };
    let Some(new) = new else {
        return Some(current);
    };
    Some(std::time::Instant::min(current, new))
}
