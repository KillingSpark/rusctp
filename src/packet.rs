use bytes::Bytes;

use crate::TransportAddress;

pub struct Packet {
    from: u16,
    to: u16,
    verification_tag: u32,
}

impl Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let from = u16::from_be_bytes(data[0..2].try_into().ok()?);
        let to = u16::from_be_bytes(data[2..4].try_into().ok()?);
        let verification_tag = u32::from_be_bytes(data[4..8].try_into().ok()?);
        let checksum = u32::from_be_bytes(data[8..12].try_into().ok()?);

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

    pub fn from(&self) -> u16 {
        self.from
    }

    pub fn to(&self) -> u16 {
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
    Init(InitChunk),
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

pub enum UnrecognizedChunkReaction {
    Stop { report: bool },
    Skip { report: bool },
}

impl UnrecognizedChunkReaction {
    pub fn report(&self) -> bool {
        match self {
            Self::Skip { report } => *report,
            Self::Stop { report } => *report,
        }
    }
}

pub struct InitChunk {
    pub aliases: Vec<TransportAddress>,
}

impl Chunk {
    pub fn parse(data: &Bytes) -> (usize, Result<Self, UnrecognizedChunkReaction>) {
        const CHUNK_HEADER_SIZE: usize = 4;
        if data.len() < CHUNK_HEADER_SIZE {
            return (
                data.len(),
                Err(UnrecognizedChunkReaction::Stop { report: false }),
            );
        }
        let typ = data[0];
        let flags = data[1];
        let len = u16::from_be_bytes(data[2..4].try_into().expect("This range is checked above"));

        let value = data.slice(CHUNK_HEADER_SIZE..);

        let kind = match typ {
            0 => ChunkKind::Data(DataSegment { buf: value }),
            1 => ChunkKind::Init(InitChunk { aliases: vec![] }),
            2 => ChunkKind::InitAck,
            3 => ChunkKind::SAck,
            4 => ChunkKind::HeartBeat,
            5 => ChunkKind::HeartBeatAck,
            6 => ChunkKind::Abort,
            7 => ChunkKind::ShutDown,
            8 => ChunkKind::ShutDownAck,
            9 => ChunkKind::OpError,
            10 => ChunkKind::StateCookie,
            11 => ChunkKind::StateCookieAck,
            12 => ChunkKind::_ReservedECNE,
            13 => ChunkKind::_ReservedCWR,
            14 => ChunkKind::ShutDownComplete,
            _ => match typ >> 6 {
                0 => {
                    return (
                        len as usize,
                        Err(UnrecognizedChunkReaction::Stop { report: false }),
                    )
                }
                1 => {
                    return (
                        len as usize,
                        Err(UnrecognizedChunkReaction::Stop { report: true }),
                    )
                }
                2 => {
                    return (
                        len as usize,
                        Err(UnrecognizedChunkReaction::Skip { report: false }),
                    )
                }
                3 => {
                    return (
                        len as usize,
                        Err(UnrecognizedChunkReaction::Skip { report: true }),
                    )
                }
                _ => unreachable!("This can onlyy have 4 values"),
            },
        };
        (len as usize, Ok(Chunk { flags, kind }))
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
