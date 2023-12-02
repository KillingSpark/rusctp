use bytes::{Buf, BufMut, Bytes};

#[cfg(feature = "async")]
pub mod assoc_sync {
    pub mod assoc;
}
#[cfg(feature = "sync")]
pub mod assoc_async {
    pub mod assoc;
}

pub mod assoc;
pub mod packet;
use assoc::{init::HandleSpecialResult, Association, RxNotification, TxNotification};
use packet::{Chunk, Packet, ParseError};

use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    hash::Hash,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
};

pub trait FakeAddr:
    Copy + Clone + PartialEq + Eq + PartialOrd + Ord + Hash + Debug + 'static
{
    fn parse(buf: &mut Bytes) -> Result<Self, ParseError>;
    fn serialize(&self, buf: &mut impl BufMut);
    fn serialized_size(&self) -> usize;
}

impl FakeAddr for u64 {
    fn serialize(&self, buf: &mut impl BufMut) {
        buf.put_u64(*self)
    }
    fn serialized_size(&self) -> usize {
        8
    }
    fn parse(buf: &mut Bytes) -> Result<Self, ParseError> {
        if buf.len() < 8 {
            return Err(ParseError::IllegalFormat);
        }
        Ok(buf.get_u64())
    }
}

impl FakeAddr for SocketAddr {
    fn parse(buf: &mut Bytes) -> Result<Self, ParseError> {
        if buf.len() < 1 {
            return Err(ParseError::IllegalFormat);
        }
        match buf.get_u8() {
            0 => {
                if buf.len() < 2 + 4 {
                    return Err(ParseError::IllegalFormat);
                }
                Ok(SocketAddr::V4(SocketAddrV4::new(
                    buf.get_u32().into(),
                    buf.get_u16(),
                )))
            }
            1 => {
                if buf.len() < 2 + 16 + 4 + 4 {
                    return Err(ParseError::IllegalFormat);
                }
                Ok(SocketAddr::V6(SocketAddrV6::new(
                    buf.get_u128().into(),
                    buf.get_u16(),
                    buf.get_u32(),
                    buf.get_u32(),
                )))
            }
            _ => {
                return Err(ParseError::IllegalFormat);
            }
        }
    }
    fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            SocketAddr::V4(v4) => {
                buf.put_u8(0);
                buf.put_u32((*v4.ip()).into());
                buf.put_u16(v4.port());
            }
            SocketAddr::V6(v6) => {
                buf.put_u8(1);
                buf.put_u128((*v6.ip()).into());
                buf.put_u16(v6.port());
                buf.put_u32(v6.flowinfo());
                buf.put_u32(v6.scope_id());
            }
        }
    }
    fn serialized_size(&self) -> usize {
        match self {
            SocketAddr::V4(_) => 1 + 2 + 4,
            SocketAddr::V6(_) => 1 + 2 + 16 + 4 + 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TransportAddress<FakeContent: FakeAddr> {
    IpV4(Ipv4Addr),
    IpV6(Ipv6Addr),
    Fake(FakeContent),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssocId(u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssocAlias<FakeContent: FakeAddr> {
    peer_addr: TransportAddress<FakeContent>,
    peer_port: u16,
    local_port: u16,
}
struct PerAssocInfo {
    local_verification_tag: u32,
    peer_verification_tag: u32,
}

struct WaitInitAck {
    local_verification_tag: u32,
    local_initial_tsn: u32,
}

struct WaitCookieAck<FakeContent: FakeAddr> {
    local_verification_tag: u32,
    peer_verification_tag: u32,
    aliases: Vec<TransportAddress<FakeContent>>,
    original_address: TransportAddress<FakeContent>,
    local_initial_tsn: u32,
    peer_initial_tsn: u32,
    local_in_streams: u16,
    peer_in_streams: u16,
    local_out_streams: u16,
    peer_out_streams: u16,
    peer_arwnd: u32,
}

#[derive(Clone)]
pub struct Settings {
    pub cookie_secret: Vec<u8>,
    pub incoming_streams: u16,
    pub outgoing_streams: u16,
    pub in_buffer_limit: usize,
    pub out_buffer_limit: usize,
    pub pmtu: usize,
}

pub struct Sctp<FakeContent: FakeAddr> {
    settings: Settings,

    assoc_id_gen: u64,

    new_assoc: Option<Association<FakeContent>>,
    assoc_infos: HashMap<AssocId, PerAssocInfo>,
    aliases: HashMap<AssocAlias<FakeContent>, AssocId>,

    wait_init_ack: HashMap<AssocAlias<FakeContent>, WaitInitAck>,
    wait_cookie_ack: HashMap<AssocAlias<FakeContent>, WaitCookieAck<FakeContent>>,

    tx_notifications: VecDeque<(AssocId, TxNotification<FakeContent>)>,
    send_immediate: VecDeque<(TransportAddress<FakeContent>, Packet, Chunk<FakeContent>)>,
    rx_notifications: VecDeque<(AssocId, RxNotification<FakeContent>)>,
}

impl<FakeContent: FakeAddr> Sctp<FakeContent> {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            assoc_id_gen: 1,

            new_assoc: None,
            assoc_infos: HashMap::new(),
            aliases: HashMap::new(),

            wait_init_ack: HashMap::new(),
            wait_cookie_ack: HashMap::new(),

            tx_notifications: VecDeque::new(),
            rx_notifications: VecDeque::new(),
            send_immediate: VecDeque::new(),
        }
    }

    pub fn receive_data(&mut self, mut data: Bytes, from: TransportAddress<FakeContent>) {
        let Some(packet) = Packet::parse(&data) else {
            return;
        };
        data.advance(12);

        // If we get an init chunk we only send the init ack and return immediatly
        if self.handle_init(&packet, &data, from).handled_or_error() {
            return;
        }

        if self
            .handle_init_ack(&packet, &mut data, from)
            .handled_or_error()
        {
            return;
        }

        let mut new_assoc_id = None;
        // Either we have accepted a new association here
        match self.handle_cookie_echo(&packet, &mut data, from) {
            HandleSpecialResult::Handled(id) => new_assoc_id = Some(id),
            HandleSpecialResult::Error => return,
            HandleSpecialResult::NotRecognized => { /* keep handling, this is allowed to carry data */
            }
        }

        // or here
        match self.handle_cookie_ack(&packet, &mut data, from) {
            HandleSpecialResult::Handled(id) => new_assoc_id = Some(id),
            HandleSpecialResult::Error => return,
            HandleSpecialResult::NotRecognized => { /* keep handling, this is allowed to carry data */
            }
        }

        // Or we need to look the ID up via the aliases
        let assoc_id = new_assoc_id.or_else(|| {
            let alias = AssocAlias {
                peer_addr: from,
                peer_port: packet.from(),
                local_port: packet.to(),
            };
            self.aliases.get(&alias).copied()
        });

        let Some(assoc_id) = assoc_id else {
            return;
        };
        self.process_chunks(assoc_id, from, &packet, data);
    }

    fn process_chunks(
        &mut self,
        assoc_id: AssocId,
        from: TransportAddress<FakeContent>,
        packet: &Packet,
        mut data: Bytes,
    ) {
        let Some(assoc_info) = self.assoc_infos.get(&assoc_id) else {
            return;
        };
        if assoc_info.local_verification_tag != packet.verification_tag() {
            return;
        }

        while !data.is_empty() {
            let (size, chunk) = Chunk::parse(&data);
            data.advance(size);

            match chunk {
                Ok(chunk) => {
                    if let Chunk::Init(_) | Chunk::InitAck(_) = chunk {
                        // TODO this is an error, init chunks may only occur as the first and single chunk in a packet
                    } else {
                        match chunk {
                            Chunk::Abort { .. } => {
                                self.aliases.retain(|_, id| *id != assoc_id);
                                // TODO delete any half open connections on abort
                                self.assoc_infos.remove(&assoc_id);
                                self.tx_notifications.retain(|(id, _)| *id != assoc_id);
                                self.rx_notifications.retain(|(id, _)| *id != assoc_id);
                                self.send_immediate.retain(|(addr, to_packet, _)| {
                                    !(*addr == from
                                        && to_packet.from() == packet.to()
                                        && to_packet.to() == packet.from())
                                });
                                self.rx_notifications
                                    .push_back((assoc_id, RxNotification::Chunk(chunk)));
                                return;
                            }
                            Chunk::HeartBeat(data) => self.send_immediate.push_back((
                                from,
                                Packet::new(
                                    packet.to(),
                                    packet.from(),
                                    assoc_info.peer_verification_tag,
                                ),
                                Chunk::HeartBeatAck(data),
                            )),
                            _ => {
                                self.rx_notifications
                                    .push_back((assoc_id, RxNotification::Chunk(chunk)));
                            }
                        }
                    }
                }
                Err(ParseError::Unrecognized { stop, report }) => {
                    if report {
                        self.send_immediate.push_back((
                            from,
                            Packet::new(
                                packet.to(),
                                packet.from(),
                                assoc_info.peer_verification_tag,
                            ),
                            Chunk::OpError,
                        ))
                    }
                    if stop {
                        break;
                    } else {
                        continue;
                    }
                }
                Err(ParseError::Done) => {
                    break;
                }
                Err(ParseError::IllegalFormat) => {
                    break;
                }
            }
        }
    }

    pub fn rx_notifications(
        &mut self,
    ) -> impl Iterator<Item = (AssocId, RxNotification<FakeContent>)> + '_ {
        self.rx_notifications.drain(..)
    }
    pub fn tx_notifications(
        &mut self,
    ) -> impl Iterator<Item = (AssocId, TxNotification<FakeContent>)> + '_ {
        self.tx_notifications.drain(..)
    }
    pub fn send_immediate(
        &mut self,
    ) -> impl Iterator<Item = (TransportAddress<FakeContent>, Packet, Chunk<FakeContent>)> + '_
    {
        self.send_immediate.drain(..)
    }
    pub fn next_send_immediate(
        &mut self,
    ) -> Option<(TransportAddress<FakeContent>, Packet, Chunk<FakeContent>)> {
        self.send_immediate.pop_front()
    }
    pub fn has_next_send_immediate(&mut self) -> bool {
        self.send_immediate.front().is_some()
    }
    pub fn new_assoc(&mut self) -> Option<Association<FakeContent>> {
        self.new_assoc.take()
    }
}
