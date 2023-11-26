#![no_main]
use libfuzzer_sys::fuzz_target;

use rusctp::packet::Chunk;
use rusctp::packet::Packet;

use bytes::Buf;
use bytes::BufMut;
use bytes::Bytes;
use bytes::BytesMut;

fuzz_target!(|data: &[u8]| {
    if let Some((packet, chunks)) = decode(data) {
        let mut buf = BytesMut::new();
        for (chunk, mut original) in chunks {
            let mut tmpbuf = BytesMut::new();
            chunk.serialize(&mut tmpbuf);

            if !tmpbuf.is_empty() {
                let tmp_type = tmpbuf[0];
                tmpbuf.advance(2);
                let original_type = original[0];
                original.advance(2);

                assert_eq!(
                    tmp_type, original_type,
                    "{chunk:?} did not serialize its type correctly back"
                );

                let tmp_len = tmpbuf.get_u16();
                let original_len = original.get_u16();

                if tmp_len == original_len {
                    let tmpbuf: &[u8] = &tmpbuf[..original_len as usize - 4];
                    let original: &[u8] = &original[..original_len as usize - 4];

                    assert_eq!(
                        tmpbuf, original,
                        "{chunk:?} did not serialize correctly back"
                    );
                    buf.put_slice(tmpbuf);
                }
            }
        }
        let buf = buf.freeze();
        let mut packet_buf = BytesMut::new();
        packet.serialize(&mut packet_buf, buf.clone());

        packet_buf.put_slice(&buf);
        let packet_buf: &[u8] = &packet_buf;
        assert_eq!(data[..8], packet_buf[..8], "{packet:?}");
    }
});

fn decode(data: &[u8]) -> Option<(Packet, Vec<(Chunk, Bytes)>)> {
    let packet = Packet::parse(data)?;

    let mut data = Bytes::copy_from_slice(&data[4..]);

    let mut chunks = vec![];
    while !data.is_empty() {
        let (size, res) = Chunk::parse(&data);
        if let Ok(chunk) = res {
            let data = data.slice(..size);
            chunks.push((chunk, data));
        }
        data.advance(size);
    }

    Some((packet, chunks))
}
