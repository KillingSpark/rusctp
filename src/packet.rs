use std::net::SocketAddr;

use crate::PortId;

pub enum PacketKind {
    Signal(Signal),
    Data(DataSegment),
}

pub enum Signal {
    HeartBeat,
    Init(PortId),
}

pub struct DataSegment {}

pub struct Packet {
    from: SocketAddr,
    to: SocketAddr,
    packet: PacketKind,
}

impl Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let _ = if data[0] == 1 {
            PacketKind::Signal(Signal::HeartBeat)
        } else {
            PacketKind::Data(DataSegment {})
        };
        todo!()
    }

    pub fn from(&self) -> SocketAddr {
        self.from
    }

    pub fn to(&self) -> SocketAddr {
        self.to
    }

    pub fn inner(self) -> PacketKind {
        self.packet
    }

    pub fn packet(&self) -> &PacketKind {
        &self.packet
    }
}
