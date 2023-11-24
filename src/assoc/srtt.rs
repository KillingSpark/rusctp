use std::time::{Duration, Instant};

use crate::packet::Tsn;

/// Implements smoothed rtt measuring and timeout calculations as defined in to rfc2988
#[derive(Debug, Clone, Copy, Default)]
pub struct Srtt {
    uninited: bool,
    measure_for_tsn: Option<(Tsn, Instant)>,
    srtt: Duration,
    rttvar: Duration,
    rto: Duration,
}

const BETA: u32 = 4;
const ALPHA: u32 = 8;

impl Srtt {
    pub fn new() -> Self {
        Self {
            uninited: true,
            measure_for_tsn: None,
            srtt: Duration::from_secs(0),
            rttvar: Duration::from_secs(0),
            rto: Duration::from_secs(0),
        }
    }

    pub fn tsn_sent(&mut self, tsn: Tsn, now: Instant) {
        if self.measure_for_tsn.is_none() {
            self.measure_for_tsn = Some((tsn, now));
        }
    }

    pub fn tsn_acked(&mut self, tsn: Tsn, now: Instant) {
        let Some((measure_for, start)) = self.measure_for_tsn else {
            return;
        };
        if measure_for <= tsn {
            self.measure_for_tsn = None;
            self.measured(now - start);
        }
    }

    fn measured(&mut self, measurement: Duration) {
        if self.uninited {
            self.uninited = false;
            self.srtt = measurement;
            self.rttvar = measurement / 2;
        } else {
            self.rttvar = (self.rttvar * (BETA - 1)) / BETA + (self.srtt - measurement) / BETA;
            self.srtt = (self.srtt * (ALPHA - 1)) / ALPHA + measurement / ALPHA;
        }
        self.rto = self.srtt + self.rttvar * 4;
    }

    pub fn rto_duration(&self) -> Duration {
        if self.uninited {
            Duration::from_secs(3)
        } else {
            self.rto
        }
    }

    pub fn rto_expired(&mut self) {
        self.rto *= 2;
    }
}
