use bytes::{Buf, BufMut, Bytes};

use crate::{
    packet::{
        cookie::{self, StateCookie},
        SupportedAddrTypes, UnrecognizedParam,
    },
    TransportAddress,
};

pub struct InitChunk {
    pub initiate_tag: u32,
    pub a_rwnd: u32,
    pub outbound_streams: u16,
    pub inbound_streams: u16,
    pub initial_tsn: u32,

    // Optional
    pub aliases: Vec<TransportAddress>,
    pub cookie_preservative_msec: Option<u32>,
    pub ecn_capable: Option<bool>,
    pub supported_addr_types: Option<SupportedAddrTypes>,
}

impl InitChunk {
    pub fn parse(mut data: Bytes) -> Option<Self> {
        if data.len() < 16 {
            return None;
        }

        let init_tag = data.get_u32();
        let a_rwnd = data.get_u32();
        let nos = data.get_u16();
        let nis = data.get_u16();
        let init_tsn = data.get_u32();

        // TODO params

        Some(Self {
            initiate_tag: init_tag,
            a_rwnd,
            outbound_streams: nos,
            inbound_streams: nis,
            initial_tsn: init_tsn,

            aliases: vec![],
            cookie_preservative_msec: None,
            ecn_capable: None,
            supported_addr_types: None,
        })
    }

    pub fn serialized_size(&self) -> usize {
        4 + 16 /*  TODO params */
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        // header
        buf.put_u8(1);
        buf.put_u8(0);
        buf.put_u16(self.serialized_size() as u16);

        // value
        buf.put_u32(self.initiate_tag);
        buf.put_u32(self.a_rwnd);
        buf.put_u16(self.outbound_streams);
        buf.put_u16(self.inbound_streams);
        buf.put_u32(self.initial_tsn);

        // TODO params
    }
}

pub struct InitAck {
    pub initiate_tag: u32,
    pub a_rwnd: u32,
    pub outbound_streams: u16,
    pub inbound_streams: u16,
    pub initial_tsn: u32,
    pub cookie: cookie::StateCookie,

    // Optional
    pub unrecognized: Vec<UnrecognizedParam>,
    pub aliases: Vec<TransportAddress>,
    pub cookie_preservative_msec: Option<u32>,
    pub ecn_capable: Option<bool>,
    pub supported_addr_types: Option<SupportedAddrTypes>,
}

impl InitAck {
    pub fn parse(mut data: Bytes) -> Option<Self> {
        if data.len() < 16 {
            return None;
        }

        let init_tag = data.get_u32();
        let a_rwnd = data.get_u32();
        let nos = data.get_u16();
        let nis = data.get_u16();
        let init_tsn = data.get_u32();

        // TODO params
        let cookie = StateCookie::parse(data);

        Some(Self {
            initiate_tag: init_tag,
            a_rwnd,
            outbound_streams: nos,
            inbound_streams: nis,
            initial_tsn: init_tsn,

            cookie,

            aliases: vec![],
            unrecognized: vec![],
            cookie_preservative_msec: None,
            ecn_capable: None,
            supported_addr_types: None,
        })
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        // header
        buf.put_u8(2);
        buf.put_u8(0);
        let serialized_cookie_size = self.cookie.serialized_size() as u16;
        buf.put_u16(4 + 16 + 4 + serialized_cookie_size);

        // value
        buf.put_u32(self.initiate_tag);
        buf.put_u32(self.a_rwnd);
        buf.put_u16(self.outbound_streams);
        buf.put_u16(self.inbound_streams);
        buf.put_u32(self.initial_tsn);

        // cookie
        buf.put_u16(7);
        buf.put_u16(serialized_cookie_size);
        self.cookie.serialize(buf);

        // TODO params
    }
}
