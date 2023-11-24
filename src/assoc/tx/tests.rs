use std::time::Instant;

use bytes::Bytes;

use crate::{
    assoc::{AssocTxSettings, AssociationTx, TxNotification},
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
    );
    let send_ten_bytes = |tx: &mut AssociationTx| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    // This should now fail as the buffer is full
    assert!(send_ten_bytes(&mut tx).is_some());

    let packet = tx
        .poll_data_to_send(100, Instant::now())
        .expect("Should return the first packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the second packet");
    tx.poll_data_to_send(100, Instant::now())
        .expect("Should return the third packet");

    assert_eq!(tx.current_in_flight, 30);
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());
    assert!(send_ten_bytes(&mut tx).is_some());

    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: packet.tsn,
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 20);
    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: packet.tsn.increase(),
                a_rwnd: 1000000, // Whatever we just want to have a big receive window here
                blocks: vec![(2, 2)],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );
    assert_eq!(tx.current_in_flight, 10);
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_some());
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
    );
    let send_ten_bytes = |tx: &mut AssociationTx| {
        tx.try_send_data(
            Bytes::copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            0,
            0,
            false,
            false,
        )
    };
    // prep packets
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());
    assert!(send_ten_bytes(&mut tx).is_none());

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());

    tx.notification(
        TxNotification::SAck((
            SelectiveAck {
                cum_tsn: Tsn(100), // Whatever we dont care about out buffer size here
                a_rwnd: 20,
                blocks: vec![],
                duplicated_tsn: vec![],
            },
            std::time::Instant::now(),
        )),
        std::time::Instant::now(),
    );

    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_some());
    assert!(tx.poll_data_to_send(100, Instant::now()).is_none());
}
