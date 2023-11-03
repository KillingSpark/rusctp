use bytes::Bytes;

use crate::PortId;

pub struct Packet {
    from: PortId,
    to: PortId,
    verification_tag: u32,
}

impl Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let from = u16::from_be_bytes(data[0..2].try_into().ok()?);
        let to = u16::from_be_bytes(data[2..4].try_into().ok()?);
        let verification_tag = u32::from_be_bytes(data[4..8].try_into().ok()?);
        let checksum = u32::from_be_bytes(data[8..12].try_into().ok()?);

        // TODO check verification tag and checksum

        let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
        let mut digest = crc.digest();
        digest.update(&data[..8]);
        digest.update(&[0, 0, 0, 0]);
        digest.update(&data[12..]);
        if !digest.finalize().eq(&checksum) {
            return None;
        }

        Some(Self {
            from,
            to,
            verification_tag,
        })
    }

    pub fn from(&self) -> PortId {
        self.from
    }

    pub fn to(&self) -> PortId {
        self.to
    }

    pub fn verification_tag(&self) -> u32 {
        self.verification_tag
    }
}

pub struct Chunk {
    flags: u8,
    kind: ChunkKind,
}

pub enum ChunkKind {
    Data(DataSegment),
    Init,
    InitAck,
    SAck,
    HeartBeat,
    HeartBeatAck,
    Abort,
    ShutDown,
    ShutDownAck,
    OpError,
    StateCookie,
    StateCookieAck,
    _ReservedECNE,
    _ReservedCWR,
    ShutDownComplete,
}

pub struct DataSegment {
    pub(crate) buf: Bytes,
}

impl Chunk {
    pub fn parse(data: &Bytes) -> Option<(usize, Self)> {
        const CHUNK_HEADER_SIZE: usize = 4;
        if data.len() < CHUNK_HEADER_SIZE {
            return None;
        }
        let typ = data[0];
        let flags = data[1];
        let len = u16::from_be_bytes(data[2..4].try_into().ok()?);

        let value = data.slice(CHUNK_HEADER_SIZE..);

        match typ {
            0 => Some(ChunkKind::Data(DataSegment { buf: value })),
            1 => Some(ChunkKind::Init),
            2 => Some(ChunkKind::InitAck),
            3 => Some(ChunkKind::SAck),
            4 => Some(ChunkKind::HeartBeat),
            5 => Some(ChunkKind::HeartBeatAck),
            6 => Some(ChunkKind::Abort),
            7 => Some(ChunkKind::ShutDown),
            8 => Some(ChunkKind::ShutDownAck),
            9 => Some(ChunkKind::OpError),
            10 => Some(ChunkKind::StateCookie),
            11 => Some(ChunkKind::StateCookieAck),
            12 => Some(ChunkKind::_ReservedECNE),
            13 => Some(ChunkKind::_ReservedCWR),
            14 => Some(ChunkKind::ShutDownComplete),
            _ => {
                // TODO this needs to check the typ and act accordingly maybe cutting the connection
                None
            }
        }
        .map(|chunk| (len as usize, Chunk { flags, kind: chunk }))
    }

    pub fn kind(&self) -> &ChunkKind {
        &self.kind
    }

    pub fn flags(&self) -> u8 {
        self.flags
    }

    pub fn into_kind(self) -> ChunkKind {
        self.kind
    }
}
