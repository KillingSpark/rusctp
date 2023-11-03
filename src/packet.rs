pub enum Packet {
    Signal(Signal),
    Data(DataSegment),
}

pub enum Signal {
    HeartBeat,
}


pub struct DataSegment {}

impl Packet {
    pub fn parse(data: &[u8]) -> Self {
        let _ = if data[0] == 1{
            Packet::Signal(Signal::HeartBeat)
        } else {
            Packet::Data(DataSegment {  })
        };
        todo!()
    }
}