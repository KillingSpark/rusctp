use bytes::{Buf, BufMut, Bytes};

use super::{param::padding_needed, CHUNK_DATA};

#[derive(PartialEq, Debug)]
pub struct DataChunk {
    pub tsn: u32,
    pub stream_id: u16,
    pub stream_seq_num: u16,
    pub ppid: u32,
    pub(crate) buf: Bytes,

    pub immediate: bool,
    pub unordered: bool,
    pub begin: bool,
    pub end: bool,
}

impl DataChunk {
    pub fn parse(flags: u8, mut data: Bytes) -> Option<Self> {
        if data.len() < 13 {
            return None;
        }
        let tsn = data.get_u32();
        let stream_id = data.get_u16();
        let stream_seq_num = data.get_u16();
        let ppid = data.get_u32();

        let immediate = flags & (0x1 << 3) == 0x1 << 3;
        let unordered = flags & (0x1 << 2) == 0x1 << 2;
        let begin = flags & (0x1 << 1) == 0x1 << 1;
        let end = flags & 0x1 == 1;

        Some(Self {
            tsn,
            stream_id,
            stream_seq_num,
            ppid,
            buf: data,

            immediate,
            unordered,
            begin,
            end,
        })
    }

    pub fn serialized_size(&self) -> usize {
        16 + self.buf.len()
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        if !self.buf.is_empty() {
            // header
            buf.put_u8(CHUNK_DATA);
            buf.put_u8(self.serialize_flags());

            let size = self.serialized_size();
            buf.put_u16(size as u16);

            // value
            buf.put_u32(self.tsn);
            buf.put_u16(self.stream_id);
            buf.put_u16(self.stream_seq_num);
            buf.put_u32(self.ppid);
            buf.put_slice(&self.buf);

            // maybe padding is needed
            buf.put_bytes(0, padding_needed(size));
        }
    }

    pub fn serialize_flags(&self) -> u8 {
        let mut flags = 0;

        if self.immediate {
            flags |= 0x1 << 3;
        }

        if self.unordered {
            flags |= 0x1 << 2;
        }

        if self.begin {
            flags |= 0x1 << 1;
        }

        if self.end {
            flags |= 0x1;
        }

        flags
    }
}
