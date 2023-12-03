use std::time::Instant;

use bytes::Bytes;

use crate::{assoc::ShutdownState, packet::Chunk, FakeAddr};

use super::AssociationTx;

#[derive(Debug, Clone, Copy)]
pub struct Timer {
    marker: u64,
    at: Instant,
}

impl Timer {
    pub fn at(&self) -> Instant {
        self.at
    }
    pub fn new(at: Instant, marker: u64) -> Self {
        Self { at, marker }
    }
}

impl Eq for Timer {}
impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.at.eq(&other.at) && self.marker.eq(&other.marker)
    }
}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(Self::cmp(self, other))
    }
}

impl Ord for Timer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.at.cmp(&other.at)
    }
}

impl<T: FakeAddr> AssociationTx<T> {
    pub fn handle_timeout(&mut self, timeout: Timer) {
        if let Some(current_timer) = self.rto_timer {
            if current_timer.marker == timeout.marker {
                self.handle_rto_timeout(timeout);
            }
        }
        if let Some(current_timer) = self.shutdown_rto_timer {
            if current_timer.marker == timeout.marker {
                self.handle_shutdown_rto_timeout(timeout);
            }
        }
        if let Some(current_timer) = self.heartbeat_timer {
            if current_timer.marker == timeout.marker {
                self.handle_heartbeat_timeout(timeout);
            }
        }
    }

    pub(super) fn handle_heartbeat_timeout(&mut self, timeout: Timer) {
        if self.pmtu_probe.probe_in_flight() {
            self.pmtu_probe.probe_timed_out();
            self.heartbeats_unacked += 1;
            // TODO we probably want to do something if heartbeats do not get answered repeatedly
        } else {
            static EMPTY_DATA: &[u8] = &[0u8; 1024 * 70];
            static BYTES: Bytes = Bytes::from_static(EMPTY_DATA);
            let probe_size = usize::min(self.pmtu_probe.next_probe_size(), BYTES.len());
            self.send_next
                .push_back(Chunk::HeartBeat(BYTES.slice(..probe_size)));
        }
        self.set_heartbeat_timeout(timeout.at);
    }

    pub(super) fn process_heartbeat_ack(&mut self, data: Bytes, now: Instant) {
        self.heartbeats_unacked = 0;
        self.pmtu_probe.probe_success(data.len());
        self.set_heartbeat_timeout(now);
    }

    pub(super) fn handle_shutdown_rto_timeout(&mut self, timeout: Timer) {
        match self.shutdown_state {
            Some(ShutdownState::ShutdownSent) => {
                self.send_next
                    .push_back(Chunk::ShutDown(self.peer_last_acked_tsn));
                self.set_shutdown_rto_timeout(timeout.at);
            }
            Some(ShutdownState::ShutdownAckSent) => {
                self.send_next.push_back(Chunk::ShutDownAck);
                self.set_shutdown_rto_timeout(timeout.at);
            }
            Some(ShutdownState::ShutdownReceived) => {}
            Some(ShutdownState::TryingTo) => {}
            Some(ShutdownState::Complete) => {}
            Some(ShutdownState::AbortReceived) => {}
            None => { /* Huh? */ }
        }
    }

    pub(super) fn handle_rto_timeout(&mut self, timeout: Timer) {
        if self.resend_queue.is_empty() {
            self.rto_timer = None;
            return;
        }
        self.primary_congestion.rto_expired();
        self.srtt.rto_expired();

        self.resend_queue.iter_mut().for_each(|p| {
            if !p.marked_for_retransmit && p.queued_at + self.srtt.rto_duration() <= timeout.at {
                p.marked_for_retransmit = true;
                p.queued_at = timeout.at;
            }
        });
        self.set_rto_timeout(timeout.at);
    }

    pub(super) fn set_heartbeat_timeout(&mut self, now: Instant) {
        let at = self.pmtu_probe.next_probe(now);
        self.heartbeat_timer = Some(Timer {
            at,
            marker: self.timer_ctr,
        });
        self.timer_ctr = self.timer_ctr.wrapping_add(1);
    }

    pub(super) fn set_rto_timeout(&mut self, now: Instant) {
        let at = now + self.srtt.rto_duration();
        self.rto_timer = Some(Timer {
            at,
            marker: self.timer_ctr,
        });
        self.timer_ctr = self.timer_ctr.wrapping_add(1);
    }

    pub(super) fn set_shutdown_rto_timeout(&mut self, now: Instant) {
        let at = now + self.srtt.rto_duration();
        self.shutdown_rto_timer = Some(Timer {
            at,
            marker: self.timer_ctr,
        });
        self.timer_ctr = self.timer_ctr.wrapping_add(1);
    }
}
