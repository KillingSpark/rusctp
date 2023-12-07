use std::{
    net::{Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use bytes::{Buf, BufMut, Bytes};

use crate::FakeAddr;

use super::{cookie::StateCookie, SupportedAddrTypes, UnrecognizedParam};

#[derive(PartialEq, Debug)]
pub enum Param<FakeContent: FakeAddr> {
    Unrecognized(UnrecognizedParam),
    StateCookie(StateCookie<FakeContent>),
    CookiePreservative(Duration),
    IpV4Addr(Ipv4Addr),
    IpV6Addr(Ipv6Addr),
    HostNameDeprecated,
    SupportedAddrTypes(SupportedAddrTypes),
}

pub(crate) const PARAM_IPV4_ADDR: u16 = 5;
pub(crate) const PARAM_IPV6_ADDR: u16 = 6;
pub(crate) const PARAM_STATE_COOKIE: u16 = 7;
pub(crate) const PARAM_UNRECOGNIZED: u16 = 8;
pub(crate) const PARAM_COOKIE_PRESERVATIVE: u16 = 9;
pub(crate) const PARAM_HOSTNAME_DEPRECATED: u16 = 11;
pub(crate) const PARAM_SUPPORTED_ADDRESSES: u16 = 12;

#[derive(Debug)]
pub enum ParseError {
    Unrecognized {
        report: bool,
        stop: bool,
        typ: u16,
        data: Bytes,
    },
    IllegalFormat,
    Done,
}

pub(crate) const PARAM_HEADER_SIZE: usize = 4;

impl<FakeContent: FakeAddr> Param<FakeContent> {
    pub(crate) fn parse(data: &Bytes) -> (usize, Result<Self, ParseError>) {
        if data.len() < PARAM_HEADER_SIZE {
            return (data.len(), Err(ParseError::Done));
        }

        let typ = u16::from_be_bytes(data[0..2].try_into().expect("This range is checked above"));
        let len = u16::from_be_bytes(data[2..4].try_into().expect("This range is checked above"));
        let len = len as usize;

        if len > data.len() {
            return (data.len(), Err(ParseError::IllegalFormat));
        }
        if len < PARAM_HEADER_SIZE {
            return (data.len(), Err(ParseError::IllegalFormat));
        }

        let mut value = data.slice(PARAM_HEADER_SIZE..len);
        let actual_len = usize::min(padded_len(len), data.len());

        let chunk = match typ {
            PARAM_IPV4_ADDR => {
                if value.len() != 4 {
                    return (actual_len, Err(ParseError::IllegalFormat));
                }
                Param::IpV4Addr(Ipv4Addr::from(value.get_u32()))
            }
            PARAM_IPV6_ADDR => {
                if value.len() != 16 {
                    return (actual_len, Err(ParseError::IllegalFormat));
                }
                Param::IpV6Addr(Ipv6Addr::from(value.get_u128()))
            }
            PARAM_COOKIE_PRESERVATIVE => {
                if value.len() != 4 {
                    return (actual_len, Err(ParseError::IllegalFormat));
                }
                Param::CookiePreservative(Duration::from_millis(value.get_u32() as u64))
            }
            PARAM_HOSTNAME_DEPRECATED => Param::HostNameDeprecated,
            PARAM_SUPPORTED_ADDRESSES => {
                let mut support = SupportedAddrTypes {
                    ipv4: false,
                    ipv6: false,
                };
                while value.len() >= 2 {
                    match value.get_u16() {
                        PARAM_IPV4_ADDR => support.ipv4 = true,
                        PARAM_IPV6_ADDR => support.ipv6 = true,
                        _ => {}
                    }
                }
                Param::SupportedAddrTypes(support)
            }
            PARAM_STATE_COOKIE => Param::StateCookie(StateCookie::Opaque(value)),
            PARAM_UNRECOGNIZED => {
                if value.len() < 4 {
                    return (actual_len, Err(ParseError::IllegalFormat));
                }
                let mut clone = value.clone();
                let typ = clone.get_u16();
                let size = clone.get_u16() as usize;
                if size != value.len() && padded_len(size) != value.len() {
                    return (actual_len, Err(ParseError::IllegalFormat));
                }
                Param::Unrecognized(UnrecognizedParam { typ, data: clone })
            }

            _ => return (actual_len, Err(parse_error(typ, data.clone()))),
        };
        (actual_len, Ok(chunk))
    }

    pub(crate) fn serialized_size(&self) -> usize {
        match self {
            Param::IpV4Addr(_) => 8,
            Param::IpV6Addr(_) => 20,
            Param::CookiePreservative(_) => 8,
            Param::HostNameDeprecated => 0,
            Param::SupportedAddrTypes(support) => {
                let mut size = PARAM_HEADER_SIZE;
                if support.ipv4 {
                    size += 2;
                }
                if support.ipv6 {
                    size += 2;
                }
                size
            }
            Param::StateCookie(cookie) => PARAM_HEADER_SIZE + cookie.serialized_size(),
            Param::Unrecognized(p) => PARAM_HEADER_SIZE + PARAM_HEADER_SIZE + p.data.len(),
        }
    }

    pub(crate) fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            Param::IpV4Addr(addr) => {
                buf.put_u16(PARAM_IPV4_ADDR);
                buf.put_u16(8);
                buf.put_u32((*addr).into());
            }
            Param::IpV6Addr(addr) => {
                buf.put_u16(PARAM_IPV6_ADDR);
                buf.put_u16(20);
                buf.put_u128((*addr).into());
            }
            Param::CookiePreservative(duration) => {
                buf.put_u16(PARAM_COOKIE_PRESERVATIVE);
                buf.put_u16(8);
                buf.put_u32(duration.as_millis() as u32);
            }
            Param::HostNameDeprecated => { /* Nope */ }
            Param::SupportedAddrTypes(support) => {
                let size = self.serialized_size();
                buf.put_u16(PARAM_SUPPORTED_ADDRESSES);
                buf.put_u16(size as u16);
                if support.ipv4 {
                    buf.put_u16(PARAM_IPV4_ADDR);
                }
                if support.ipv6 {
                    buf.put_u16(PARAM_IPV6_ADDR);
                }
                // maybe padding is needed
                buf.put_bytes(0, padding_needed(size));
            }
            Param::StateCookie(cookie) => cookie.serialize_as_param(buf),
            Param::Unrecognized(p) => p.serialize_as_param(buf),
        }
    }
}

pub fn padding_needed(size: usize) -> usize {
    (4 - (size % 4)) % 4
}

pub fn padded_len(size: usize) -> usize {
    ((size + 3) / 4) * 4
}

fn parse_error(typ: u16, data: Bytes) -> ParseError {
    let stop;
    let report;
    match typ >> 14 {
        0 => {
            stop = true;
            report = false;
        }
        1 => {
            stop = true;
            report = true;
        }
        2 => {
            stop = false;
            report = false;
        }
        3 => {
            stop = false;
            report = true;
        }
        _ => unreachable!("This can onlyy have 4 values"),
    }
    ParseError::Unrecognized {
        report,
        stop,
        typ,
        data,
    }
}

#[test]
fn roundtrip() {
    use crate::{packet::cookie::Cookie, TransportAddress};

    fn roundtrip(param: Param<u64>) {
        let mut buf = bytes::BytesMut::new();
        param.serialize(&mut buf);
        assert_eq!(buf.len(), padded_len(param.serialized_size()));
        let buf = buf.freeze();
        let mut clone = buf.clone();
        let _typ = clone.get_u16();
        let size = clone.get_u16() as usize;
        assert_eq!(param.serialized_size(), size);

        let (size, deserialized) = Param::parse(&buf);
        assert_eq!(size, padded_len(param.serialized_size()));

        let mut deserialized = deserialized.unwrap();
        if matches!(param, Param::StateCookie(StateCookie::Ours(_))) {
            if let Param::StateCookie(ref mut cookie) = deserialized {
                cookie.make_ours().unwrap();
            }
        }
        assert_eq!(param, deserialized);
    }

    roundtrip(Param::IpV4Addr(127001.into()));
    roundtrip(Param::IpV6Addr(1234567.into()));
    roundtrip(Param::CookiePreservative(Duration::from_millis(1000)));
    roundtrip(Param::SupportedAddrTypes(SupportedAddrTypes {
        ipv4: false,
        ipv6: false,
    }));
    roundtrip(Param::SupportedAddrTypes(SupportedAddrTypes {
        ipv4: true,
        ipv6: false,
    }));
    roundtrip(Param::SupportedAddrTypes(SupportedAddrTypes {
        ipv4: false,
        ipv6: true,
    }));
    roundtrip(Param::SupportedAddrTypes(SupportedAddrTypes {
        ipv4: true,
        ipv6: true,
    }));
    roundtrip(Param::Unrecognized(UnrecognizedParam {
        typ: 100,
        data: Bytes::copy_from_slice(&[0, 100, 0, 4]),
    }));
    roundtrip(Param::StateCookie(StateCookie::Opaque(
        Bytes::copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]),
    )));
    roundtrip(Param::StateCookie(StateCookie::Ours(Cookie {
        mac: 1234,
        init_address: TransportAddress::IpV6(1234.into()),
        peer_port: 1234,
        local_port: 1234,
        aliases: vec![
            TransportAddress::IpV4(1234.into()),
            TransportAddress::IpV4(12341.into()),
            TransportAddress::IpV6(1234.into()),
            TransportAddress::IpV6(12341.into()),
        ],
        local_verification_tag: 1234,
        peer_verification_tag: 1234,
        local_initial_tsn: 1234,
        peer_initial_tsn: 1234,
        incoming_streams: 1234,
        outgoing_streams: 1234,
        peer_arwnd: 1234,
    })));
}
