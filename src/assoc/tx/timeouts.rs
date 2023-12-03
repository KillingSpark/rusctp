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
        // TODO perform pmtu testing with this
        // https://datatracker.ietf.org/doc/rfc8899/
        self.send_next
            .push_back(Chunk::HeartBeat(Bytes::from_static(&[])));
        self.set_heartbeat_timeout(timeout.at);
        self.heartbeats_unacked += 1;
        // TODO we probably want to do something if heartbeats do not get answered
    }

    pub(super) fn process_heartbeat_ack(&mut self, _data: Bytes, now: Instant) {
        // TODO pmtu checking?
        self.heartbeats_unacked = 0;
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
        let at = now + self.srtt.rto_duration();
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
