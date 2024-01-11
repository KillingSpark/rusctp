use std::time::Instant;

use crate::{
    packet::{sack::SelectiveAck, Tsn},
    FakeAddr,
};

use super::AssociationTx;

impl<T: FakeAddr> AssociationTx<T> {
    fn process_sack_gap_blocks(
        &mut self,
        sack: &SelectiveAck,
        fully_acked: usize,
        in_flight_before_sack: usize,
    ) {
        let mut partial_bytes_acked = 0;
        let mut block_iter = sack.blocks.iter().map(|(start, end)| {
            (
                Tsn(sack.cum_tsn.0 + *start as u32),
                Tsn(sack.cum_tsn.0 + *end as u32),
            )
        });
        let mut queue_iter = self.resend_queue.iter_mut();

        let mut next_block = block_iter.next();
        let mut next_packet = queue_iter.next();
        while let Some((block_range, packet)) = next_block.zip(next_packet) {
            if packet.partially_acked {
                next_packet = queue_iter.next();
            } else if packet.data.tsn < block_range.0 {
                // Missing packet
                // TODO we may only mark as many packets as fit in a PMTU and only once when we enter fast recovery
                packet.marked_for_fast_retransmit = true;
                next_packet = queue_iter.next();
            } else if packet.data.tsn >= block_range.0 && packet.data.tsn < block_range.1 {
                // Packet received
                // TODO what about sacks that contain this range multiple times?
                partial_bytes_acked += packet.data.buf.len();
                packet.partially_acked = true;
                next_packet = queue_iter.next();
            } else {
                // take next block
                next_block = block_iter.next();
                next_packet = Some(packet);
            }
        }
        // eprintln!("Got sack up to: {:?} New bytes acked: {fully_acked} {sack:?}", sack.cum_tsn);
        // eprintln!("Still in queues: rtx {} out {}", self.resend_queue.len(), self.out_queue.len());
        self.primary_congestion.bytes_acked(
            partial_bytes_acked,
            fully_acked,
            in_flight_before_sack,
        );
    }

    pub(super) fn handle_sack(&mut self, sack: SelectiveAck, now: Instant) {
        let in_flight_before_sack = self.current_in_flight;

        if self.last_acked_tsn > sack.cum_tsn {
            // This is a reordered sack, we can safely ignore this
            return;
        }
        if sack.cum_tsn == self.last_acked_tsn {
            self.peer_rcv_window = sack.a_rwnd.saturating_sub(self.current_in_flight as u32);
            self.duplicated_acks += 1;
            if self.duplicated_acks >= 2 {
                self.primary_congestion.enter_fast_recovery();
            }
            self.process_sack_gap_blocks(&sack, 0, in_flight_before_sack);
            return;
        }

        self.last_acked_tsn = sack.cum_tsn;
        self.duplicated_acks = 0;

        let mut bytes_acked = 0;

        while self
            .resend_queue
            .front()
            .map(|packet| packet.data.tsn <= sack.cum_tsn)
            .unwrap_or(false)
        {
            let acked = self.resend_queue.pop_front().unwrap();
            self.current_out_buffered -= acked.data.buf.len();
            self.current_in_flight -= acked.data.buf.len();
            bytes_acked += acked.data.buf.len();
        }

        self.process_sack_gap_blocks(&sack, bytes_acked, in_flight_before_sack);

        self.peer_rcv_window = sack.a_rwnd.saturating_sub(self.current_in_flight as u32);
        self.srtt.tsn_acked(sack.cum_tsn, now);
        if bytes_acked > 0 {
            if self.current_in_flight > 0 {
                self.set_rto_timeout(now);
            } else {
                self.rto_timer = None;
            }
        }
    }
}
