use crate::PortId;

pub struct Packet {
    from: PortId,
    to: PortId,
    verification_tag: u32,
    checksum: u32,
}

impl Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let from = u16::from_be_bytes(data[0..2].try_into().ok()?);
        let to = u16::from_be_bytes(data[2..4].try_into().ok()?);
        let verification_tag = u32::from_be_bytes(data[4..8].try_into().ok()?);
        let checksum = u32::from_be_bytes(data[8..12].try_into().ok()?);

        // TODO check verification tag and checksum

        Some(Self {
            from,
            to,
            verification_tag,
            checksum,
        })
    }

    pub fn from(&self) -> PortId {
        self.from
    }

    pub fn to(&self) -> PortId {
        self.to
    }
}

pub enum Chunk {
    Signal(Signal),
    Data(DataSegment),
}

pub enum Signal {
    HeartBeat,
    Init(PortId),
}

pub struct DataSegment {}

impl Chunk {
    pub fn parse(_data: &[u8]) -> Option<(usize, Self)> {
        todo!()
    }
}