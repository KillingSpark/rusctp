use std::hash::Hash;
use std::hash::Hasher;

use bytes::Buf;
use bytes::BufMut;
use bytes::Bytes;

use crate::TransportAddress;

use super::param::padding_needed;
use super::param::PARAM_HEADER_SIZE;
use super::param::PARAM_STATE_COOKIE;

pub enum StateCookie {
    Ours(Cookie),
    Opaque(Bytes),
}

pub struct Cookie {
    pub mac: u64,
    pub init_address: TransportAddress,
    pub peer_port: u16,
    pub local_port: u16,
    pub aliases: Vec<TransportAddress>,
    pub local_verification_tag: u32,
    pub peer_verification_tag: u32,
    pub local_initial_tsn: u32,
    pub peer_initial_tsn: u32,
}

impl StateCookie {
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
            Self::Opaque(data) => buf.put_slice(&data),
            Self::Ours(cookie) => cookie.serialize(buf),
        }
    }

    pub fn make_ours(&mut self) -> Option<&Cookie> {
        match self {
            Self::Ours(cookie) => Some(cookie),
            Self::Opaque(data) => {
                *self = StateCookie::Ours(Cookie::parse(data.clone())?);
                self.make_ours()
            },
        }
    }
}

impl Cookie {
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
            init_address,
            aliases,
        } = self;

        (*init_address).hash(&mut hasher);
        (*aliases).hash(&mut hasher);
        (*peer_port).hash(&mut hasher);
        (*local_port).hash(&mut hasher);
        (*peer_verification_tag).hash(&mut hasher);
        (*local_verification_tag).hash(&mut hasher);
        (*peer_initial_tsn).hash(&mut hasher);
        (*local_initial_tsn).hash(&mut hasher);
        hasher.write(local_secret);
        hasher.finish()
    }

    pub fn serialized_size(&self) -> usize {
        let mut size = 4 + 4 + 4 + 4 + 2 + 2 + 8;

        match self.init_address {
            TransportAddress::Fake(_) => {
                size += 1;
                size += 8;
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
                TransportAddress::Fake(_) => {
                    size += 1;
                    size += 8;
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
            init_address,
            aliases,
        } = self;
        buf.put_u64(*mac);
        buf.put_u16(*peer_port);
        buf.put_u16(*local_port);
        buf.put_u32(*peer_verification_tag);
        buf.put_u32(*local_verification_tag);
        buf.put_u32(*peer_initial_tsn);
        buf.put_u32(*local_initial_tsn);

        match init_address {
            TransportAddress::Fake(addr) => {
                buf.put_u8(0);
                buf.put_u64(*addr);
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
                TransportAddress::Fake(addr) => {
                    buf.put_u8(0);
                    buf.put_u64(*addr);
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
        }
    }

    pub fn parse(mut data: Bytes) -> Option<Self> {
        if data.remaining() < 29 {
            return None;
        }
        let mac = data.get_u64();
        let peer_port = data.get_u16();
        let local_port = data.get_u16();
        let peer_verification_tag = data.get_u32();
        let local_verification_tag = data.get_u32();
        let peer_initial_tsn = data.get_u32();
        let local_initial_tsn = data.get_u32();
        let init_address;
        let mut aliases = vec![];

        match data.get_u8() {
            0 => {
                if data.remaining() < 8 {
                    return None;
                }
                init_address = TransportAddress::Fake(data.get_u64());
            }
            1 => {
                if data.remaining() < 4 {
                    return None;
                }
                init_address = TransportAddress::IpV4(data.get_u32().into());
            }
            2 => {
                if data.remaining() < 16 {
                    return None;
                }
                init_address = TransportAddress::IpV4(data.get_u32().into());
            }
            _ => {
                return None;
            }
        }

        while data.remaining() > 5 {
            match data.get_u8() {
                0 => {
                    if data.remaining() < 8 {
                        return None;
                    }
                    aliases.push(TransportAddress::Fake(data.get_u64()));
                }
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
                    aliases.push(TransportAddress::IpV4(data.get_u32().into()));
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
            init_address,
            aliases,
        })
    }
}
