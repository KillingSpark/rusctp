use bytes::{Buf, BufMut, Bytes};

use super::{param::padding_needed, CHUNK_HEADER_SIZE, CHUNK_SACK};

#[derive(PartialEq, Debug)]
pub struct SelectiveAck {
    pub cum_tsn: u32,
    pub a_rwnd: u32,
    pub blocks: Vec<(u16, u16)>,
    pub duplicated_tsn: Vec<u32>,
}

impl SelectiveAck {
    pub fn parse(mut data: Bytes) -> Option<Self> {
        if data.len() < 4 + 4 + 2 + 2 {
            return None;
        }
        let cum_tsn = data.get_u32();
        let a_rwnd = data.get_u32();
        let n_blocks = data.get_u16();
        let n_dups = data.get_u16();
        let mut blocks = vec![];
        let mut duplicated_tsn = vec![];

        if data.len() < 4 * (n_blocks + n_dups) as usize {
            return None;
        }
        for _ in 0..n_blocks {
            blocks.push((data.get_u16(), data.get_u16()));
        }
        for _ in 0..n_dups {
            duplicated_tsn.push(data.get_u32());
        }

        Some(Self {
            cum_tsn,
            a_rwnd,
            blocks,
            duplicated_tsn,
        })
    }

    pub fn serialized_size(&self) -> usize {
        CHUNK_HEADER_SIZE + 4 + 4 + 2 + 2 + self.blocks.len() * 4 + self.duplicated_tsn.len() * 4
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        // header
        buf.put_u8(CHUNK_SACK);
        buf.put_u8(0);

        let size = self.serialized_size();
        buf.put_u16(size as u16);

        // value
        buf.put_u32(self.cum_tsn);
        buf.put_u32(self.a_rwnd);
        buf.put_u16(self.blocks.len() as u16);
        buf.put_u16(self.duplicated_tsn.len() as u16);

        for block in &self.blocks {
            buf.put_u16(block.0);
            buf.put_u16(block.1);
        }

        for dup in &self.duplicated_tsn {
            buf.put_u32(*dup);
        }

        // maybe padding is needed
        buf.put_bytes(0, padding_needed(size));
    }
}
