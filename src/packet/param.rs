use std::{
    net::{Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use bytes::{Buf, Bytes};

use super::{cookie::StateCookie, SupportedAddrTypes, UnrecognizedParam};

pub enum Param {
    Unrecognized(UnrecognizedParam),
    StateCookie(StateCookie),
    CookiePreservative(Duration),
    IpV4Addr(Ipv4Addr),
    IpV6Addr(Ipv6Addr),
    HostNameDeprecated,
    SupportedAddrTypes(SupportedAddrTypes),
}

const PARAM_IPV4_ADDR: u16 = 5;
const PARAM_IPV6_ADDR: u16 = 6;
const PARAM_STATE_COOKIE: u16 = 7;
const PARAM_UNRECOGNIZED: u16 = 8;
const PARAM_COOKIE_PRESERVATIVE: u16 = 9;
const PARAM_HOSTNAME_DEPRECATED: u16 = 11;
const PARAM_SUPPORTED_ADDRESSES: u16 = 12;

pub enum ParseError {
    Unrecognized {
        report: bool,
        stop: bool,
        data: Bytes,
    },
    IllegalFormat,
    Done,
}

const PARAM_HEADER_SIZE: usize = 4;

impl Param {
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

        let mut value = data.slice(PARAM_HEADER_SIZE..len);
        let padded_len = usize::min(len + len % 4, data.len());

        let chunk = match typ {
            PARAM_IPV4_ADDR => {
                if value.len() != 4 {
                    return (padded_len, Err(ParseError::IllegalFormat));
                }
                Param::IpV4Addr(Ipv4Addr::from(value.get_u32()))
            }
            PARAM_IPV6_ADDR => {
                if value.len() != 16 {
                    return (padded_len, Err(ParseError::IllegalFormat));
                }
                Param::IpV6Addr(Ipv6Addr::from(value.get_u128()))
            }
            PARAM_COOKIE_PRESERVATIVE => {
                if value.len() != 4 {
                    return (padded_len, Err(ParseError::IllegalFormat));
                }
                Param::CookiePreservative(Duration::from_millis(value.get_u32() as u64))
            }
            PARAM_HOSTNAME_DEPRECATED => Param::HostNameDeprecated,
            PARAM_SUPPORTED_ADDRESSES => {
                let mut support = SupportedAddrTypes {
                    ipv4: false,
                    ipv6: false,
                };
                while value.len() > 2 {
                    match value.get_u16() {
                        PARAM_IPV4_ADDR => support.ipv4 = true,
                        PARAM_IPV6_ADDR => support.ipv6 = true,
                        _ => {}
                    }
                }
                Param::SupportedAddrTypes(support)
            }
            PARAM_STATE_COOKIE => Param::StateCookie(StateCookie::Opaque(value)),
            PARAM_UNRECOGNIZED => Param::Unrecognized(UnrecognizedParam {
                typ,
                data: data.clone(),
            }),

            _ => return (padded_len, Err(parse_error(typ, data.clone()))),
        };
        (padded_len, Ok(chunk))
    }
}

fn parse_error(typ: u16, data: Bytes) -> ParseError {
    match typ >> 14 {
        0 => ParseError::Unrecognized {
            stop: true,
            report: false,
            data,
        },
        1 => ParseError::Unrecognized {
            stop: true,
            report: true,
            data,
        },
        2 => ParseError::Unrecognized {
            stop: false,
            report: false,
            data,
        },
        3 => ParseError::Unrecognized {
            stop: true,
            report: true,
            data,
        },
        _ => unreachable!("This can onlyy have 4 values"),
    }
}
