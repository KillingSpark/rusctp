use std::hash::Hash;
use std::hash::Hasher;

use bytes::BufMut;
use bytes::Bytes;

use crate::TransportAddress;

pub struct StateCookie {
    pub mac: u64,
    pub init_address: TransportAddress,
    pub peer_port: u16,
    pub local_port: u16,
    pub aliases: Vec<TransportAddress>,
}

impl StateCookie {
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

    pub fn parse(_data: Bytes) -> Option<Self> {
        todo!()
    }

    pub fn serialized_size(&self) -> usize {
        todo!()
    }

    pub fn serialize(&self, _buf: &mut impl BufMut) -> usize {
        todo!()
    }
}
