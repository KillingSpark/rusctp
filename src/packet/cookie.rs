use std::hash::Hash;
use std::hash::Hasher;

use bytes::Buf;
use bytes::BufMut;
use bytes::Bytes;

use crate::FakeAddr;
use crate::TransportAddress;

use super::param::padding_needed;
use super::param::PARAM_HEADER_SIZE;
use super::param::PARAM_STATE_COOKIE;

#[derive(PartialEq, Debug)]
pub enum StateCookie<FakeContent: FakeAddr> {
    Ours(Cookie<FakeContent>),
    Opaque(Bytes),
}

#[derive(PartialEq, Debug)]
pub struct Cookie<FakeContent: FakeAddr> {
    pub mac: u64,
    pub init_address: TransportAddress<FakeContent>,
    pub peer_port: u16,
    pub local_port: u16,
    pub aliases: Vec<TransportAddress<FakeContent>>,
    pub local_verification_tag: u32,
    pub peer_verification_tag: u32,
    pub local_initial_tsn: u32,
    pub peer_initial_tsn: u32,
    pub incoming_streams: u16,
    pub outgoing_streams: u16,
    pub peer_arwnd: u32,
}

impl<FakeContent: FakeAddr> StateCookie<FakeContent> {
    pub fn parse(data: Bytes) -> Self {
        Self::Opaque(data)
    }

    pub fn serialized_size(&self) -> usize {
        match self {
            Self::Opaque(data) => data.len(),
            Self::Ours(cookie) => cookie.serialized_size(),
        }
    }

    pub fn serialize_as_param(&self, buf: &mut impl BufMut) {
        let size = PARAM_HEADER_SIZE + self.serialized_size();
        buf.put_u16(PARAM_STATE_COOKIE);
        buf.put_u16(size as u16);
        self.serialize(buf);
        // maybe padding is needed
        buf.put_bytes(0, padding_needed(size));
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            Self::Opaque(data) => buf.put_slice(data),
            Self::Ours(cookie) => cookie.serialize(buf),
        }
    }

    pub fn make_ours(&mut self) -> Option<&Cookie<FakeContent>> {
        match self {
            Self::Ours(cookie) => Some(cookie),
            Self::Opaque(data) => {
                *self = StateCookie::Ours(Cookie::parse(data.clone())?);
                self.make_ours()
            }
        }
    }
}

impl<FakeContent: FakeAddr> Cookie<FakeContent> {
    pub fn calc_mac(&self, local_secret: &[u8]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        let Cookie {
            mac: _,
            peer_port,
            local_port,
            peer_verification_tag,
            local_verification_tag,
            peer_initial_tsn,
            local_initial_tsn,
            incoming_streams,
            outgoing_streams,
            init_address,
            aliases,
            peer_arwnd,
        } = self;

        (*init_address).hash(&mut hasher);
        (*aliases).hash(&mut hasher);
        (*peer_port).hash(&mut hasher);
        (*local_port).hash(&mut hasher);
        (*peer_verification_tag).hash(&mut hasher);
        (*local_verification_tag).hash(&mut hasher);
        (*peer_initial_tsn).hash(&mut hasher);
        (*local_initial_tsn).hash(&mut hasher);
        (*incoming_streams).hash(&mut hasher);
        (*outgoing_streams).hash(&mut hasher);
        (*peer_arwnd).hash(&mut hasher);
        hasher.write(local_secret);
        hasher.finish()
    }

    pub fn serialized_size(&self) -> usize {
        let mut size = 4 + 4 + 4 + 4 + 2 + 2 + 8 + 2 + 2 + 4;

        match self.init_address {
            TransportAddress::Fake(addr) => {
                size += 1;
                size += addr.serialized_size();
            }
            TransportAddress::IpV4(_) => {
                size += 1;
                size += 4;
            }
            TransportAddress::IpV6(_) => {
                size += 1;
                size += 16;
            }
        }

        for addr in &self.aliases {
            match addr {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(_) => {
                    size += 1;
                    size += 4;
                }
                TransportAddress::IpV6(_) => {
                    size += 1;
                    size += 16;
                }
            }
        }

        size
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        let Cookie {
            mac,
            peer_port,
            local_port,
            peer_verification_tag,
            local_verification_tag,
            peer_initial_tsn,
            local_initial_tsn,
            incoming_streams,
            outgoing_streams,
            init_address,
            aliases,
            peer_arwnd,
        } = self;
        buf.put_u64(*mac);
        buf.put_u16(*peer_port);
        buf.put_u16(*local_port);
        buf.put_u32(*peer_verification_tag);
        buf.put_u32(*local_verification_tag);
        buf.put_u32(*peer_initial_tsn);
        buf.put_u32(*local_initial_tsn);
        buf.put_u16(*incoming_streams);
        buf.put_u16(*outgoing_streams);
        buf.put_u32(*peer_arwnd);

        match init_address {
            TransportAddress::Fake(addr) => {
                buf.put_u8(0);
                addr.serialize(buf);
            }
            TransportAddress::IpV4(addr) => {
                buf.put_u8(1);
                buf.put_u32((*addr).into());
            }
            TransportAddress::IpV6(addr) => {
                buf.put_u8(2);
                buf.put_u128((*addr).into());
            }
        }

        for addr in aliases {
            match addr {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(addr) => {
                    buf.put_u8(1);
                    buf.put_u32((*addr).into());
                }
                TransportAddress::IpV6(addr) => {
                    buf.put_u8(2);
                    buf.put_u128((*addr).into());
                }
            }
        }
    }

    pub fn parse(mut data: Bytes) -> Option<Self> {
        if data.remaining() < 33 {
            return None;
        }
        let mac = data.get_u64();
        let peer_port = data.get_u16();
        let local_port = data.get_u16();
        let peer_verification_tag = data.get_u32();
        let local_verification_tag = data.get_u32();
        let peer_initial_tsn = data.get_u32();
        let local_initial_tsn = data.get_u32();
        let incoming_streams = data.get_u16();
        let outgoing_streams = data.get_u16();
        let peer_arwnd = data.get_u32();

        let mut aliases = vec![];

        let init_address = match data.get_u8() {
            0 => TransportAddress::Fake(FakeContent::parse(&mut data).ok()?),
            1 => {
                if data.remaining() < 4 {
                    return None;
                }
                TransportAddress::IpV4(data.get_u32().into())
            }
            2 => {
                if data.remaining() < 16 {
                    return None;
                }
                TransportAddress::IpV6(data.get_u128().into())
            }
            _ => {
                return None;
            }
        };

        while data.remaining() >= 5 {
            match data.get_u8() {
                1 => {
                    if data.remaining() < 4 {
                        return None;
                    }
                    aliases.push(TransportAddress::IpV4(data.get_u32().into()));
                }
                2 => {
                    if data.remaining() < 16 {
                        return None;
                    }
                    aliases.push(TransportAddress::IpV6(data.get_u128().into()));
                }
                _ => {
                    return None;
                }
            }
        }

        Some(Self {
            mac,
            peer_port,
            local_port,
            peer_verification_tag,
            local_verification_tag,
            peer_initial_tsn,
            local_initial_tsn,
            incoming_streams,
            outgoing_streams,
            init_address,
            aliases,
            peer_arwnd,
        })
    }
}

#[test]
fn roundtrip() {
    use bytes::BytesMut;

    let cookie = Cookie {
        mac: 0,
        init_address: TransportAddress::Fake(12345u64),
        peer_port: 12334,
        local_port: 12335,
        aliases: vec![
            TransportAddress::IpV4(12345.into()),
            TransportAddress::IpV6(12345.into()),
            TransportAddress::IpV4(123451.into()),
            TransportAddress::IpV6(123451.into()),
        ],
        local_verification_tag: 1234,
        peer_verification_tag: 5467,
        local_initial_tsn: 9012,
        peer_initial_tsn: 3456,
        incoming_streams: 1234,
        outgoing_streams: 1234,
        peer_arwnd: 1234,
    };

    let mut buf = BytesMut::new();
    cookie.serialize(&mut buf);
    assert_eq!(buf.len(), cookie.serialized_size());
    let parsed = Cookie::parse(buf.freeze()).unwrap();
    assert_eq!(cookie, parsed);
}
