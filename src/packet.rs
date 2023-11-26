use std::cmp::Ordering;

use self::{
    cookie::StateCookie,
    data::DataChunk,
    init::{InitAck, InitChunk},
    param::{padded_len, padding_needed, PARAM_HEADER_SIZE, PARAM_UNRECOGNIZED},
    sack::SelectiveAck,
};
use bytes::{Buf, BufMut, Bytes};

pub mod cookie;
pub mod data;
pub mod init;
pub mod param;
pub mod sack;

#[derive(Clone, Copy, Debug)]
pub struct Packet {
    from: u16,
    to: u16,
    verification_tag: u32,
}

static CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);

impl Packet {
    pub fn new(from: u16, to: u16, verification_tag: u32) -> Self {
        Self {
            to,
            from,
            verification_tag,
        }
    }

    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }
        let from = u16::from_be_bytes(data[0..2].try_into().ok()?);
        let to = u16::from_be_bytes(data[2..4].try_into().ok()?);
        let verification_tag = u32::from_be_bytes(data[4..8].try_into().ok()?);
        let checksum = u32::from_be_bytes(data[8..12].try_into().ok()?);

        #[cfg(not(feature = "fuzz"))]
        {
            let mut digest = CRC.digest();
            digest.update(&data[..8]);
            digest.update(&[0, 0, 0, 0]);
            digest.update(&data[12..]);
            if !digest.finalize().eq(&checksum) {
                return None;
            }
        }
        #[cfg(feature = "fuzz")]
        let _ = checksum;

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

        let mut digest = CRC.digest();
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

#[derive(PartialEq, Debug)]
pub enum Chunk {
    Data(DataChunk),
    Init(init::InitChunk),
    InitAck(init::InitAck),
    SAck(sack::SelectiveAck),
    HeartBeat(Bytes),
    HeartBeatAck(Bytes),
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

#[derive(Debug, Clone, Copy)]
pub struct Sequence(pub u16);

impl Sequence {
    pub fn increase(self) -> Self {
        Sequence(self.0.wrapping_add(1))
    }
    pub fn decrease(self) -> Self {
        Sequence(self.0.wrapping_sub(1))
    }
}

impl Eq for Sequence {}

impl PartialEq for Sequence {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl Ord for Sequence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        const WRAP_RAD: u16 = u16::MAX / 128;
        const WRAP_HI: u16 = u16::MAX - WRAP_RAD;
        const WRAP_LO: u16 = WRAP_RAD;
        if self.0 < WRAP_LO && other.0 > WRAP_HI {
            Ordering::Greater
        } else if other.0 < WRAP_LO && self.0 > WRAP_HI {
            Ordering::Less
        } else {
            self.0.cmp(&other.0)
        }
    }
}

impl PartialOrd for Sequence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(Self::cmp(self, other))
    }
}

#[test]
fn sequence_compare() {
    // Normal ordering should work as expected
    assert_eq!(Ordering::Less, Sequence(1).cmp(&Sequence(2)));
    assert_eq!(Ordering::Greater, Sequence(2).cmp(&Sequence(1)));
    assert_eq!(Ordering::Equal, Sequence(1).cmp(&Sequence(1)));

    // But inside the wrap radius we should have wrapping compare
    assert_eq!(Ordering::Less, Sequence(u16::MAX).cmp(&Sequence(1)));
    assert_eq!(Ordering::Greater, Sequence(1).cmp(&Sequence(u16::MAX)));
}

#[derive(Debug, Clone, Copy)]
pub struct Tsn(pub u32);

impl Tsn {
    pub fn increase(self) -> Self {
        Tsn(self.0.wrapping_add(1))
    }
    pub fn decrease(self) -> Self {
        Tsn(self.0.wrapping_sub(1))
    }
}

impl Eq for Tsn {}

impl PartialEq for Tsn {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl Ord for Tsn {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        const WRAP_RAD: u32 = u32::MAX / 128;
        const WRAP_HI: u32 = u32::MAX - WRAP_RAD;
        const WRAP_LO: u32 = WRAP_RAD;
        if self.0 < WRAP_LO && other.0 > WRAP_HI {
            Ordering::Greater
        } else if other.0 < WRAP_LO && self.0 > WRAP_HI {
            Ordering::Less
        } else {
            self.0.cmp(&other.0)
        }
    }
}

impl PartialOrd for Tsn {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(Self::cmp(self, other))
    }
}

#[test]
fn tsn_compare() {
    // Normal ordering should work as expected
    assert_eq!(Ordering::Less, Tsn(1).cmp(&Tsn(2)));
    assert_eq!(Ordering::Greater, Tsn(2).cmp(&Tsn(1)));
    assert_eq!(Ordering::Equal, Tsn(1).cmp(&Tsn(1)));

    // But inside the wrap radius we should have wrapping compare
    assert_eq!(Ordering::Less, Tsn(u32::MAX).cmp(&Tsn(1)));
    assert_eq!(Ordering::Greater, Tsn(1).cmp(&Tsn(u32::MAX)));
}

#[derive(PartialEq, Debug)]
pub struct UnrecognizedParam {
    pub typ: u16,
    pub data: Bytes,
}

impl UnrecognizedParam {
    pub fn serialize_as_param(&self, buf: &mut impl BufMut) {
        buf.put_u16(PARAM_UNRECOGNIZED);
        buf.put_u16((PARAM_HEADER_SIZE + PARAM_HEADER_SIZE + self.data.len()) as u16);
        buf.put_u16(self.typ);
        buf.put_u16((PARAM_HEADER_SIZE + self.data.len()) as u16);
        buf.put_slice(&self.data);
        // maybe padding is needed
        buf.put_bytes(0, padding_needed(self.data.len()));
    }
}

#[derive(Debug)]
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
            data[0] == CHUNK_INIT
        }
    }

    pub fn is_init_ack(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == CHUNK_INIT_ACK
        }
    }

    pub fn is_cookie_echo(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == CHUNK_STATE_COOKIE
        }
    }

    pub fn is_cookie_ack(data: &[u8]) -> bool {
        if data.len() < CHUNK_HEADER_SIZE {
            false
        } else {
            data[0] == CHUNK_STATE_COOKIE_ACK
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
        if len < CHUNK_HEADER_SIZE {
            return (data.len(), Err(ParseError::IllegalFormat));
        }

        let value = data.slice(CHUNK_HEADER_SIZE..len);
        let padded_len = usize::min(padded_len(len), data.len());

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
            CHUNK_SACK => {
                let Some(sack) = SelectiveAck::parse(value) else {
                    return (padded_len, Err(ParseError::IllegalFormat));
                };
                Chunk::SAck(sack)
            }
            CHUNK_HEARTBEAT => Chunk::HeartBeat(value),
            CHUNK_HEARTBEAT_ACK => Chunk::HeartBeatAck(value),
            CHUNK_ABORT => Chunk::Abort,
            CHUNK_SHUTDOWN => Chunk::ShutDown,
            CHUNK_SHUTDOWN_ACK => Chunk::ShutDownAck,
            CHUNK_OP_ERROR => Chunk::OpError,
            CHUNK_STATE_COOKIE => Chunk::StateCookie(StateCookie::Opaque(value)),
            CHUNK_STATE_COOKIE_ACK => {
                if !value.is_empty() {
                    return (padded_len, Err(ParseError::IllegalFormat));
                }
                Chunk::StateCookieAck
            }
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
            Chunk::Init(init) => init.serialize(buf),
            Chunk::InitAck(ack) => ack.serialize(buf),
            Chunk::StateCookie(cookie) => {
                buf.put_u8(CHUNK_STATE_COOKIE);
                buf.put_u8(0);
                let size = cookie.serialized_size();
                buf.put_u16((CHUNK_HEADER_SIZE + size) as u16);
                cookie.serialize(buf);
                // maybe padding is needed
                buf.put_bytes(0, padding_needed(size));
            }
            Chunk::SAck(sack) => sack.serialize(buf),
            Chunk::HeartBeat(data) => {
                buf.put_u8(CHUNK_HEARTBEAT);
                buf.put_u8(0);
                let size = data.len() + CHUNK_HEADER_SIZE;
                buf.put_u16(size as u16);
                buf.put_slice(data);
                // maybe padding is needed
                buf.put_bytes(0, padding_needed(size));
            }
            Chunk::HeartBeatAck(data) => {
                buf.put_u8(CHUNK_HEARTBEAT_ACK);
                buf.put_u8(0);
                let size = data.len() + CHUNK_HEADER_SIZE;
                buf.put_u16(size as u16);
                buf.put_slice(data);
                // maybe padding is needed
                buf.put_bytes(0, padding_needed(size));
            }
            _ => {
                #[cfg(not(feature = "fuzz"))]
                unimplemented!();
            }
        }
    }

    pub fn serialized_size(&self) -> usize {
        match self {
            Chunk::Data(data) => data.serialized_size(),
            Chunk::StateCookieAck => COOKIE_ACK_BYTES.len(),
            Chunk::Init(init) => init.serialized_size(),
            Chunk::InitAck(ack) => ack.serialized_size(),
            Chunk::StateCookie(cookie) => CHUNK_HEADER_SIZE + cookie.serialized_size(),
            Chunk::SAck(sack) => sack.serialized_size(),
            Chunk::HeartBeat(data) => CHUNK_HEADER_SIZE + data.len(),
            Chunk::HeartBeatAck(data) => CHUNK_HEADER_SIZE + data.len(),
            _ => {
                #[cfg(not(feature = "fuzz"))]
                unimplemented!();
                #[cfg(feature = "fuzz")]
                0
            }
        }
    }
    pub fn padded_serialized_size(&self) -> usize {
        padded_len(self.serialized_size())
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

#[test]
fn roundtrip() {
    use crate::TransportAddress;
    use std::time::Duration;

    fn roundtrip(chunk: Chunk) {
        let mut buf = bytes::BytesMut::new();
        chunk.serialize(&mut buf);
        assert_eq!(buf.len(), padded_len(chunk.serialized_size()));
        let buf = buf.freeze();
        let mut clone = buf.clone();
        let _typ = clone.get_u16();
        let size = clone.get_u16() as usize;
        assert_eq!(chunk.serialized_size(), size);

        let (size, deserialized) = Chunk::parse(&buf);
        assert_eq!(size, padded_len(chunk.serialized_size()));

        let deserialized = deserialized.unwrap();
        assert_eq!(chunk, deserialized);
    }

    roundtrip(Chunk::Data(DataChunk {
        tsn: Tsn(1234),
        stream_id: 1234,
        stream_seq_num: Sequence(1234),
        ppid: 1234,
        buf: Bytes::copy_from_slice(&[1, 2, 3, 4, 5, 6]),
        immediate: true,
        unordered: false,
        begin: true,
        end: false,
    }));
    roundtrip(Chunk::Init(InitChunk {
        initiate_tag: 1234,
        a_rwnd: 1234,
        outbound_streams: 1234,
        inbound_streams: 1234,
        initial_tsn: 1234,
        unrecognized: vec![], // intentionally won't get serialized
        aliases: vec![TransportAddress::IpV4(10.into())],
        cookie_preservative: Some(Duration::from_millis(400)),
        supported_addr_types: Some(SupportedAddrTypes {
            ipv4: true,
            ipv6: false,
        }),
    }));

    roundtrip(Chunk::InitAck(InitAck {
        initiate_tag: 1234,
        a_rwnd: 1234,
        outbound_streams: 1234,
        inbound_streams: 1234,
        initial_tsn: 1234,
        unrecognized: vec![UnrecognizedParam {
            typ: 100,
            data: Bytes::copy_from_slice(&[0, 100, 0, 4]),
        }],
        aliases: vec![TransportAddress::IpV4(10.into())],
        cookie_preservative: Some(Duration::from_millis(400)),
        supported_addr_types: Some(SupportedAddrTypes {
            ipv4: true,
            ipv6: false,
        }),
        cookie: StateCookie::Opaque(Bytes::copy_from_slice(&[100, 101, 102, 255, 0])),
    }));

    roundtrip(Chunk::StateCookie(StateCookie::Opaque(
        Bytes::copy_from_slice(&[100, 101, 102, 255, 0]),
    )));

    roundtrip(Chunk::StateCookieAck);

    roundtrip(Chunk::SAck(SelectiveAck {
        cum_tsn: Tsn(1234),
        a_rwnd: 1234,
        blocks: vec![(1, 2), (3, 4)],
        duplicated_tsn: vec![1, 2, 3, 4],
    }));
}
