use std::time::Duration;

use bytes::{Buf, BufMut, Bytes};

use crate::{
    packet::{
        cookie::{self},
        SupportedAddrTypes, UnrecognizedParam,
    },
    TransportAddress,
};

use super::param::{padded_len, padding_needed, Param, ParseError, PARAM_HEADER_SIZE};

#[derive(PartialEq, Debug)]
pub struct InitChunk {
    pub initiate_tag: u32,
    pub a_rwnd: u32,
    pub outbound_streams: u16,
    pub inbound_streams: u16,
    pub initial_tsn: u32,

    pub unrecognized: Vec<UnrecognizedParam>,

    // Optional
    pub aliases: Vec<TransportAddress>,
    pub cookie_preservative: Option<Duration>,
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

        let mut this = Self {
            initiate_tag: init_tag,
            a_rwnd,
            outbound_streams: nos,
            inbound_streams: nis,
            initial_tsn: init_tsn,

            unrecognized: vec![],

            aliases: vec![],
            cookie_preservative: None,
            supported_addr_types: None,
        };

        while !data.is_empty() {
            let (size, param) = Param::parse(&data);
            data.advance(size);
            match param {
                Ok(param) => match param {
                    Param::IpV4Addr(addr) => {
                        this.aliases.push(TransportAddress::IpV4(addr));
                    }
                    Param::IpV6Addr(addr) => {
                        this.aliases.push(TransportAddress::IpV6(addr));
                    }
                    Param::SupportedAddrTypes(support) => {
                        this.supported_addr_types = Some(support);
                    }
                    Param::HostNameDeprecated => {
                        // TODO react properly to deprecated param
                    }
                    Param::CookiePreservative(duration) => {
                        this.cookie_preservative = Some(duration)
                    }
                    _ => {
                        // TODO react properly to param that doesnt belong here
                    }
                },
                Err(ParseError::Done) => {
                    break;
                }
                Err(ParseError::IllegalFormat) => {
                    return None;
                }
                Err(ParseError::Unrecognized {
                    report,
                    stop,
                    data,
                    typ,
                }) => {
                    if report {
                        this.unrecognized.push(UnrecognizedParam { typ, data })
                    }
                    if stop {
                        break;
                    }
                }
            }
        }

        Some(this)
    }

    pub fn serialized_size(&self) -> usize {
        let mut size = 4 + 16;
        for alias in &self.aliases {
            match alias {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(addr) => {
                    size += padded_len(Param::IpV4Addr(*addr).serialized_size())
                }
                TransportAddress::IpV6(addr) => {
                    size += padded_len(Param::IpV6Addr(*addr).serialized_size())
                }
            }
        }

        if let Some(duration) = self.cookie_preservative {
            size += padded_len(Param::CookiePreservative(duration).serialized_size());
        }

        if let Some(suppoert) = self.supported_addr_types {
            size += padded_len(Param::SupportedAddrTypes(suppoert).serialized_size());
        }

        size
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        // header
        buf.put_u8(1);
        buf.put_u8(0);
        let size = self.serialized_size();
        buf.put_u16(size as u16);

        // value
        buf.put_u32(self.initiate_tag);
        buf.put_u32(self.a_rwnd);
        buf.put_u16(self.outbound_streams);
        buf.put_u16(self.inbound_streams);
        buf.put_u32(self.initial_tsn);

        for alias in &self.aliases {
            match alias {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(addr) => Param::IpV4Addr(*addr).serialize(buf),
                TransportAddress::IpV6(addr) => Param::IpV6Addr(*addr).serialize(buf),
            }
        }

        if let Some(duration) = self.cookie_preservative {
            Param::CookiePreservative(duration).serialize(buf);
        }

        if let Some(suppoert) = self.supported_addr_types {
            Param::SupportedAddrTypes(suppoert).serialize(buf);
        }

        // maybe padding is needed
        buf.put_bytes(0, padding_needed(size));
    }
}

#[derive(PartialEq, Debug)]
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
    pub cookie_preservative: Option<Duration>,
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

        let mut aliases = vec![];
        let mut unrecognized = vec![];
        let mut cookie_preservative = None;
        let mut supported_addr_types = None;

        let mut cookie = None;

        while !data.is_empty() {
            let (size, param) = Param::parse(&data);
            data.advance(size);
            match param {
                Ok(param) => match param {
                    Param::IpV4Addr(addr) => {
                        aliases.push(TransportAddress::IpV4(addr));
                    }
                    Param::IpV6Addr(addr) => {
                        aliases.push(TransportAddress::IpV6(addr));
                    }
                    Param::SupportedAddrTypes(support) => {
                        supported_addr_types = Some(support);
                    }
                    Param::HostNameDeprecated => {
                        // TODO react properly to deprecated param
                    }
                    Param::CookiePreservative(duration) => cookie_preservative = Some(duration),
                    Param::Unrecognized(p) => unrecognized.push(p),
                    Param::StateCookie(c) => cookie = Some(c),
                },
                Err(ParseError::Done) => {
                    break;
                }
                Err(ParseError::IllegalFormat) => {
                    return None;
                }
                Err(ParseError::Unrecognized {
                    report,
                    stop,
                    data,
                    typ,
                }) => {
                    if report {
                        _ = data;
                        _ = typ;
                        // TODO report data back
                    }
                    if stop {
                        break;
                    }
                }
            }
        }

        let Some(cookie) = cookie else {
            return None;
        };

        Some(Self {
            initiate_tag: init_tag,
            a_rwnd,
            outbound_streams: nos,
            inbound_streams: nis,
            initial_tsn: init_tsn,

            cookie,

            aliases,
            unrecognized,
            cookie_preservative,
            supported_addr_types,
        })
    }

    pub fn serialized_size(&self) -> usize {
        let mut size = 4 + 16 + 4 + padded_len(self.cookie.serialized_size());
        for alias in &self.aliases {
            match alias {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(addr) => {
                    size += padded_len(Param::IpV4Addr(*addr).serialized_size())
                }
                TransportAddress::IpV6(addr) => {
                    size += padded_len(Param::IpV6Addr(*addr).serialized_size())
                }
            }
        }

        if let Some(duration) = self.cookie_preservative {
            size += padded_len(Param::CookiePreservative(duration).serialized_size());
        }

        if let Some(support) = self.supported_addr_types {
            size += padded_len(Param::SupportedAddrTypes(support).serialized_size());
        }

        for unrecognized in &self.unrecognized {
            size += padded_len(PARAM_HEADER_SIZE + PARAM_HEADER_SIZE + unrecognized.data.len());
        }

        size
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        // header
        buf.put_u8(2);
        buf.put_u8(0);
        let size = self.serialized_size();
        buf.put_u16(size as u16);

        // value
        buf.put_u32(self.initiate_tag);
        buf.put_u32(self.a_rwnd);
        buf.put_u16(self.outbound_streams);
        buf.put_u16(self.inbound_streams);
        buf.put_u32(self.initial_tsn);

        // cookie
        self.cookie.serialize_as_param(buf);

        for alias in &self.aliases {
            match alias {
                TransportAddress::Fake(_) => {}
                TransportAddress::IpV4(addr) => Param::IpV4Addr(*addr).serialize(buf),
                TransportAddress::IpV6(addr) => Param::IpV6Addr(*addr).serialize(buf),
            }
        }

        for unrecognized in &self.unrecognized {
            unrecognized.serialize_as_param(buf)
        }

        if let Some(duration) = self.cookie_preservative {
            Param::CookiePreservative(duration).serialize(buf);
        }

        if let Some(suppoert) = self.supported_addr_types {
            Param::SupportedAddrTypes(suppoert).serialize(buf);
        }

        // maybe padding is needed
        buf.put_bytes(0, padding_needed(size));
    }
}
