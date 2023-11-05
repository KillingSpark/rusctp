use bytes::{Buf, Bytes};

pub struct DataSegment {
    tsn: u32,
    stream_id: u16,
    stream_seq_num: u16,
    ppid: u32,
    pub(crate) buf: Bytes,

    immediate: bool,
    unordered: bool,
    begin: bool,
    end: bool,
}

impl DataSegment {
    pub fn parse(flags: u8, mut data: Bytes) -> Option<Self> {
        if data.len() < 13 {
            return None;
        }
        let tsn = data.get_u32();
        let stream_id = data.get_u16();
        let stream_seq_num = data.get_u16();
        let ppid = data.get_u32();

        let immediate = (flags >> 3) & 0x1 == 1;
        let unordered = (flags >> 2) & 0x1 == 1;
        let begin = (flags >> 1) & 0x1 == 1;
        let end = flags & 0x1 == 1;

        Some(Self {
            tsn,
            stream_id,
            stream_seq_num,
            ppid,
            buf: data.slice(12..),

            immediate,
            unordered,
            begin,
            end,
        })
    }
}
