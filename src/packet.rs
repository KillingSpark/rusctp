use self::{
    cookie::StateCookie,
    data::DataChunk,
    init::{InitAck, InitChunk},
    param::{padding_needed, PARAM_HEADER_SIZE, PARAM_UNRECOGNIZED},
};
use bytes::{Buf, BufMut, Bytes};

pub mod cookie;
pub mod data;
pub mod init;
pub mod param;

pub struct Packet {
    from: u16,
    to: u16,
    verification_tag: u32,
}

impl Packet {
    pub fn new(from: u16, to: u16, verification_tag: u32) -> Self {
        Self {
            to,
            from,
            verification_tag,
        }
    }

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

    pub fn serialize(&self, buf: &mut impl BufMut, mut chunks: impl Buf) {
        buf.put_u16(self.from);
        buf.put_u16(self.to);
        buf.put_u32(self.verification_tag);

        let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
        let mut digest = crc.digest();
        digest.update(&self.from.to_be_bytes());
        digest.update(&self.to.to_be_bytes());
        digest.update(&self.verification_tag.to_be_bytes());
        digest.update(&[0, 0, 0, 0]);
        while chunks.has_remaining() {
            let chunk = chunks.chunk();
            if chunk.is_empty() {
                break;
            }
            digest.update(chunk);
            chunks.advance(chunk.len());
        }

        buf.put_u32(digest.finalize());
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
    Data(DataChunk),
    Init(init::InitChunk),
    InitAck(init::InitAck),
    SAck,
    HeartBeat,
    HeartBeatAck,
    Abort,
    ShutDown,
    ShutDownAck,
    OpError,
    StateCookie(cookie::StateCookie),
    StateCookieAck,
    _ReservedECNE,
    _ReservedCWR,
    ShutDownComplete,
}

#[derive(PartialEq, Debug)]
pub struct UnrecognizedParam {
    pub typ: u16,
    pub data: Bytes,
}

impl UnrecognizedParam {
    pub fn serialize_as_param(&self, buf: &mut impl BufMut) {
        buf.put_u16(PARAM_UNRECOGNIZED);
        buf.put_u16((PARAM_HEADER_SIZE + self.data.len()) as u16);
        buf.put_slice(&self.data);
        // maybe padding is needed
        buf.put_bytes(0, padding_needed(self.data.len()));
    }
}

pub enum ParseError {
    Unrecognized { report: bool, stop: bool },
    IllegalFormat,
    Done,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SupportedAddrTypes {
    ipv4: bool,
    ipv6: bool,
}

static COOKIE_ACK_BYTES: &[u8] = &[CHUNK_STATE_COOKIE_ACK, 0, 0, 4];

pub(crate) const CHUNK_DATA: u8 = 0;
pub(crate) const CHUNK_INIT: u8 = 1;
pub(crate) const CHUNK_INIT_ACK: u8 = 2;
pub(crate) const CHUNK_SACK: u8 = 3;
pub(crate) const CHUNK_HEARTBEAT: u8 = 4;
pub(crate) const CHUNK_HEARTBEAT_ACK: u8 = 5;
pub(crate) const CHUNK_ABORT: u8 = 6;
pub(crate) const CHUNK_SHUTDOWN: u8 = 7;
pub(crate) const CHUNK_SHUTDOWN_ACK: u8 = 8;
pub(crate) const CHUNK_OP_ERROR: u8 = 9;
pub(crate) const CHUNK_STATE_COOKIE: u8 = 10;
pub(crate) const CHUNK_STATE_COOKIE_ACK: u8 = 11;
pub(crate) const CHUNK_RESERVED_ECNE: u8 = 12;
pub(crate) const CHUNK_RESERVED_CWR: u8 = 13;
pub(crate) const CHUNK_SHUTDOWN_COMPLETE: u8 = 14;

const CHUNK_HEADER_SIZE: usize = 4;
impl Chunk {
    pub fn is_init(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == 1
        }
    }

    pub fn is_init_ack(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == 2
        }
    }

    pub fn is_cookie_echo(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == 10
        }
    }

    pub fn parse(data: &Bytes) -> (usize, Result<Self, ParseError>) {
        if data.len() < CHUNK_HEADER_SIZE {
            return (data.len(), Err(ParseError::Done));
        }
        let typ = data[0];
        let flags = data[1];
        let len = u16::from_be_bytes(data[2..4].try_into().expect("This range is checked above"));
        let len = len as usize;

        if len > data.len() {
            return (data.len(), Err(ParseError::IllegalFormat));
        }

        let value = data.slice(CHUNK_HEADER_SIZE..len);
        let padded_len = usize::min(len + len % 4, data.len());

        let chunk = match typ {
            CHUNK_DATA => {
                let Some(data) = DataChunk::parse(flags, value) else {
                    return (padded_len, Err(ParseError::IllegalFormat));
                };
                Chunk::Data(data)
            }
            CHUNK_INIT => {
                let Some(init) = InitChunk::parse(value) else {
                    return (padded_len, Err(ParseError::IllegalFormat));
                };
                Chunk::Init(init)
            }
            CHUNK_INIT_ACK => {
                let Some(init) = InitAck::parse(value) else {
                    return (padded_len, Err(ParseError::IllegalFormat));
                };
                Chunk::InitAck(init)
            }
            CHUNK_SACK => Chunk::SAck,
            CHUNK_HEARTBEAT => Chunk::HeartBeat,
            CHUNK_HEARTBEAT_ACK => Chunk::HeartBeatAck,
            CHUNK_ABORT => Chunk::Abort,
            CHUNK_SHUTDOWN => Chunk::ShutDown,
            CHUNK_SHUTDOWN_ACK => Chunk::ShutDownAck,
            CHUNK_OP_ERROR => Chunk::OpError,
            CHUNK_STATE_COOKIE => Chunk::StateCookie(StateCookie::Opaque(value)),
            CHUNK_STATE_COOKIE_ACK => Chunk::StateCookieAck,
            CHUNK_RESERVED_ECNE => Chunk::_ReservedECNE,
            CHUNK_RESERVED_CWR => Chunk::_ReservedCWR,
            CHUNK_SHUTDOWN_COMPLETE => Chunk::ShutDownComplete,
            _ => return (padded_len, Err(parse_error(typ))),
        };
        (padded_len, Ok(chunk))
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            Chunk::Data(data) => data.serialize(buf),
            Chunk::StateCookieAck => buf.put_slice(COOKIE_ACK_BYTES),
            _ => unimplemented!(),
        }
    }

    pub fn serialized_size(&self) -> usize {
        match self {
            Chunk::Data(data) => data.serialized_size(),
            Chunk::StateCookieAck => COOKIE_ACK_BYTES.len(),
            _ => unimplemented!(),
        }
    }

    pub fn cookie_ack_bytes() -> Bytes {
        Bytes::from_static(COOKIE_ACK_BYTES)
    }
}

fn parse_error(typ: u8) -> ParseError {
    match typ >> 6 {
        0 => ParseError::Unrecognized {
            stop: true,
            report: false,
        },
        1 => ParseError::Unrecognized {
            stop: true,
            report: true,
        },
        2 => ParseError::Unrecognized {
            stop: false,
            report: false,
        },
        3 => ParseError::Unrecognized {
            stop: false,
            report: true,
        },
        _ => unreachable!("This can onlyy have 4 values"),
    }
}
