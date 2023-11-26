#![no_main]
use libfuzzer_sys::fuzz_target;

use rusctp::packet::Chunk;
use rusctp::packet::Packet;

use bytes::Buf;

fuzz_target!(|data: &[u8]| {
    decode(data);
});

fn decode(data: &[u8]) -> Option<(Packet, Vec<Chunk>)> {
    let packet = Packet::parse(data)?;

    let mut data = bytes::Bytes::copy_from_slice(&data[4..]);

    let mut chunks = vec![];
    while !data.is_empty() {
        let (size, res) = Chunk::parse(&data);
        data.advance(size);
        if let Ok(chunk) = res {
            chunks.push(chunk);
        }
    }

    Some((packet, chunks))
}
