use std::hash::Hash;
use std::hash::Hasher;

use bytes::BufMut;
use bytes::Bytes;

use crate::TransportAddress;

use super::param::padding_needed;
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
}

impl StateCookie {
    pub fn parse(data: Bytes) -> Self {
        Self::Opaque(data)
    }

    pub fn serialized_size(&self) -> usize {
        todo!()
    }

    pub fn serialize_as_param(&self, buf: &mut impl BufMut) {
        let size = self.serialized_size();
        buf.put_u16(PARAM_STATE_COOKIE);
        buf.put_u16(size as u16);
        self.serialize(buf);
        // maybe padding is needed
        buf.put_bytes(0, padding_needed(size));
    }

    pub fn serialize(&self, _buf: &mut impl BufMut) {
        todo!()
    }

    pub fn make_ours(&mut self) -> Option<&Cookie> {
        match self {
            Self::Ours(cookie) => Some(cookie),
            Self::Opaque(_) => todo!(),
        }
    }
}

impl Cookie {
    pub fn calc_mac(
        init_address: TransportAddress,
        aliases: &[TransportAddress],
        peer_port: u16,
        local_port: u16,
        local_secret: &[u8],
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        init_address.hash(&mut hasher);
        aliases.hash(&mut hasher);
        peer_port.hash(&mut hasher);
        local_port.hash(&mut hasher);
        hasher.write(local_secret);
        hasher.finish()
    }
}
