use bytes::{BufMut, Bytes};

use crate::TransportAddress;

use self::{
    cookie::{Cookie, StateCookie},
    data::DataChunk,
    init::{InitAck, InitChunk},
};

pub mod cookie;
pub mod data;
pub mod init;

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

    pub fn serialize(&self, buf: &mut impl BufMut, chunks: &Bytes) {
        buf.put_u16(self.from);
        buf.put_u16(self.to);
        buf.put_u32(self.verification_tag);

        let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
        let mut digest = crc.digest();
        digest.update(&self.from.to_be_bytes());
        digest.update(&self.to.to_be_bytes());
        digest.update(&self.verification_tag.to_be_bytes());
        digest.update(&[0, 0, 0, 0]);
        digest.update(chunks);

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

pub struct UnrecognizedParam {
    pub typ: u8,
}

pub enum ParseError {
    Unrecognized { report: bool, stop: bool },
    IllegalFormat,
    Done,
}

pub enum SupportedAddrTypes {
    IpV4,
    IpV6,
    IpV4and6,
}

static COOKIE_ACK_BYTES: &[u8] = &[11, 0, 0, 4];

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

        let value = data.slice(CHUNK_HEADER_SIZE..CHUNK_HEADER_SIZE + len);

        let chunk = match typ {
            0 => {
                let Some(data) = DataChunk::parse(flags, value) else {
                    return (len, Err(ParseError::IllegalFormat));
                };
                Chunk::Data(data)
            }
            1 => {
                let Some(init) = InitChunk::parse(value) else {
                    return (len, Err(ParseError::IllegalFormat));
                };
                Chunk::Init(init)
            }
            2 => {
                let Some(init) = InitAck::parse(value) else {
                    return (len, Err(ParseError::IllegalFormat));
                };
                Chunk::InitAck(init)
            }
            3 => Chunk::SAck,
            4 => Chunk::HeartBeat,
            5 => Chunk::HeartBeatAck,
            6 => Chunk::Abort,
            7 => Chunk::ShutDown,
            8 => Chunk::ShutDownAck,
            9 => Chunk::OpError,
            10 => Chunk::StateCookie(StateCookie::Ours(Cookie {
                aliases: vec![],
                init_address: TransportAddress::Fake(100),
                peer_port: 10,
                local_port: 10,
                mac: 100,
            })),
            11 => Chunk::StateCookieAck,
            12 => Chunk::_ReservedECNE,
            13 => Chunk::_ReservedCWR,
            14 => Chunk::ShutDownComplete,
            _ => match typ >> 6 {
                0 => {
                    return (
                        len,
                        Err(ParseError::Unrecognized {
                            stop: true,
                            report: false,
                        }),
                    )
                }
                1 => {
                    return (
                        len,
                        Err(ParseError::Unrecognized {
                            stop: true,
                            report: true,
                        }),
                    )
                }
                2 => {
                    return (
                        len,
                        Err(ParseError::Unrecognized {
                            stop: false,
                            report: false,
                        }),
                    )
                }
                3 => {
                    return (
                        len,
                        Err(ParseError::Unrecognized {
                            stop: true,
                            report: true,
                        }),
                    )
                }
                _ => unreachable!("This can onlyy have 4 values"),
            },
        };
        (len, Ok(chunk))
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
