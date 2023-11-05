use std::hash::Hasher;

use bytes::{BufMut, Bytes, BytesMut};

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

pub enum Chunk {
    Data(DataSegment),
    Init(InitChunk),
    InitAck(InitChunk, StateCookie),
    SAck,
    HeartBeat,
    HeartBeatAck,
    Abort,
    ShutDown,
    ShutDownAck,
    OpError,
    StateCookie(StateCookie),
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

pub struct StateCookie {
    pub init_address: TransportAddress,
    pub aliases: Vec<TransportAddress>,
    pub peer_port: u16,
    pub local_port: u16,
    pub mac: u64,
}

impl StateCookie {
    pub fn calc_mac(
        init_address: TransportAddress,
        aliases: &[TransportAddress],
        peer_port: u16,
        local_port: u16,
        local_secret: &[u8],
    ) -> u64 {
        use std::hash::Hash;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        init_address.hash(&mut hasher);
        aliases.hash(&mut hasher);
        peer_port.hash(&mut hasher);
        local_port.hash(&mut hasher);
        hasher.write(local_secret);
        hasher.finish()
    }
}

static COOKIE_ACK_BYTES: &'static [u8] = &[11, 0, 0, 4];

const CHUNK_HEADER_SIZE: usize = 4;
impl Chunk {
    pub fn is_init(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == 1
        }
    }

    pub fn is_cookie_echo(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == 10
        }
    }

    pub fn parse(data: &Bytes) -> (usize, Result<Self, UnrecognizedChunkReaction>) {
        if data.len() < CHUNK_HEADER_SIZE {
            return (
                data.len(),
                Err(UnrecognizedChunkReaction::Stop { report: false }),
            );
        }
        let typ = data[0];
        let _flags = data[1];
        let len = u16::from_be_bytes(data[2..4].try_into().expect("This range is checked above"));

        let value = data.slice(CHUNK_HEADER_SIZE..);

        let chunk = match typ {
            0 => Chunk::Data(DataSegment { buf: value }),
            1 => Chunk::Init(InitChunk { aliases: vec![] }),
            2 => Chunk::InitAck(
                InitChunk { aliases: vec![] },
                StateCookie {
                    aliases: vec![],
                    init_address: TransportAddress::Fake(100),
                    peer_port: 10,
                    local_port: 10,
                    mac: 100,
                },
            ),
            3 => Chunk::SAck,
            4 => Chunk::HeartBeat,
            5 => Chunk::HeartBeatAck,
            6 => Chunk::Abort,
            7 => Chunk::ShutDown,
            8 => Chunk::ShutDownAck,
            9 => Chunk::OpError,
            10 => Chunk::StateCookie(StateCookie {
                aliases: vec![],
                init_address: TransportAddress::Fake(100),
                peer_port: 10,
                local_port: 10,
                mac: 100,
            }),
            11 => Chunk::StateCookieAck,
            12 => Chunk::_ReservedECNE,
            13 => Chunk::_ReservedCWR,
            14 => Chunk::ShutDownComplete,
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
        (len as usize, Ok(chunk))
    }

    pub fn serialize(&self, buf: &mut BytesMut) {
        match self {
            Chunk::StateCookieAck => buf.put_slice(COOKIE_ACK_BYTES),
            _ => unimplemented!(),
        }
    }

    pub fn cookie_ack_bytes() -> Bytes {
        Bytes::from_static(COOKIE_ACK_BYTES)
    }
}
