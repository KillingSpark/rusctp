use std::time::Instant;

use bytes::Bytes;

use crate::{
    assoc::{AssocTxSettings, AssociationTx, SendError, SendErrorKind, TxNotification},
    packet::{sack::SelectiveAck, Tsn},
    AssocId, TransportAddress,
};

#[test]
fn buffer_limits() {
    let mut tx = AssociationTx::new(
        AssocId(0),
        AssocTxSettings {
            primary_path: TransportAddress::Fake(0),
            peer_verification_tag: 1234,
            local_port: 1,
            peer_port: 2,
            init_tsn: Tsn(1),
            out_streams: 1,
            out_buffer_limit: 100,
            peer_arwnd: 10000000,
            pmtu: 10000,
        },
        Instant::now(),
    );
    let send_ten_bytes = |tx: &mut AssociationTx<u64>| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    // This should now fail as the buffer is full
    assert!(matches!(
        send_ten_bytes(&mut tx),
        Err(SendError {
            data: _,
            kind: SendErrorKind::BufferFull
        })
    ));

    let packet = tx
        .poll_data_to_send(100, Instant::now())
        .expect("Should return the first packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the second packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the third packet");

    assert_eq!(tx.current_in_flight, 30);
    assert!(matches!(
        send_ten_bytes(&mut tx),
        Err(SendError {
            data: _,
            kind: SendErrorKind::BufferFull
        })
    ));
    assert!(matches!(
        send_ten_bytes(&mut tx),
        Err(SendError {
            data: _,
            kind: SendErrorKind::BufferFull
        })
    ));
    assert!(matches!(
        send_ten_bytes(&mut tx),
        Err(SendError {
            data: _,
            kind: SendErrorKind::BufferFull
        })
    ));

    tx.notification(
        TxNotification::SAck(
            SelectiveAck {
                cum_tsn: packet.tsn,
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        ),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 20);
    tx.notification(
        TxNotification::SAck(
            SelectiveAck {
                cum_tsn: packet.tsn.increase(),
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        ),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 10);
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(matches!(
        send_ten_bytes(&mut tx),
        Err(SendError {
            data: _,
            kind: SendErrorKind::BufferFull
        })
    ));
}

#[test]
fn arwnd_limits() {
    let mut tx = AssociationTx::new(
        AssocId(0),
        AssocTxSettings {
            primary_path: TransportAddress::Fake(0),
            peer_verification_tag: 1234,
            local_port: 1,
            peer_port: 2,
            init_tsn: Tsn(1),
            out_streams: 1,
            out_buffer_limit: 100,
            peer_arwnd: 20,
            pmtu: 10000,
        },
        Instant::now(),
    );
    let send_ten_bytes = |tx: &mut AssociationTx<u64>| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    // prep packets
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());
    assert!(send_ten_bytes(&mut tx).is_ok());

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    tx.notification(
        TxNotification::SAck(
            SelectiveAck {
                cum_tsn: Tsn(100), // Whatever we dont care about out buffer size here
                a_rwnd: 20,
                blocks: vec![],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        ),
        std::time::Instant::now(),
    );

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());
}

#[test]
fn rto_timeout() {
    let mut tx = AssociationTx::new(
        AssocId(0),
        AssocTxSettings {
            primary_path: TransportAddress::Fake(0),
            peer_verification_tag: 1234,
            local_port: 1,
            peer_port: 2,
            init_tsn: Tsn(1),
            out_streams: 1,
            out_buffer_limit: 100,
            peer_arwnd: 20,
            pmtu: 10000,
        },
        Instant::now(),
    );
    let send_ten_bytes = |tx: &mut AssociationTx<u64>| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    // prep packet
    assert!(send_ten_bytes(&mut tx).is_ok());

    // take first, this is the "original" transmission
    let first = tx.poll_data_to_send(100, Instant::now()).unwrap();
    // shouldn't retransmit before timeout has run out
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    // time it out
    let timeout = tx.next_timeout().unwrap();
    tx.handle_timeout(timeout);

    // now we should get a retransmission
    let second = tx.poll_data_to_send(100, Instant::now()).unwrap();
    // which is equal to the original transmission
    assert_eq!(first, second);
    // shouldn't retransmit before timeout has run out again
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    // give it an old timeout, this should be ignored
    tx.handle_timeout(timeout);
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    // timeout for real this time
    tx.handle_timeout(tx.next_timeout().unwrap());
    // now we should get a retransmission
    let third = tx.poll_data_to_send(100, Instant::now()).unwrap();
    // which is equal to the other transmissions
    assert_eq!(second, third);
}
