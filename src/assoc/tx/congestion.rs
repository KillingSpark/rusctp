#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CongestionState {
    SlowStart,
    FastRecovery,
    CongestionAvoidance,
    LossRecovery,
}

pub struct PerDestinationInfo {
    pub state: CongestionState,
    pub pmcds: usize,
    pub cwnd: usize,
    ssthresh: usize,
    partial_bytes_acked: usize,
}

impl PerDestinationInfo {
    pub fn new(pmtu: usize) -> Self {
        let pmcds = pmtu - 12;
        Self {
            state: CongestionState::SlowStart,
            pmcds,
            cwnd: usize::min(4 * pmcds, usize::max(2 * pmcds, 4404)),
            ssthresh: usize::MAX,
            partial_bytes_acked: 0,
        }
    }

    pub fn bytes_acked(
        &mut self,
        partial_bytes_acked: usize,
        bytes_acked: usize,
        in_flight_before_sack: usize,
    ) {
        match self.state {
            CongestionState::FastRecovery | CongestionState::LossRecovery => {
                if bytes_acked > 0 {
                    // (Fast-) recovery did move the tsn, go back to slowstart
                    self.change_state(CongestionState::SlowStart);
                }
            }
            CongestionState::CongestionAvoidance => {
                self.partial_bytes_acked += bytes_acked + partial_bytes_acked;
                if self.partial_bytes_acked >= self.cwnd {
                    if in_flight_before_sack < self.cwnd {
                        self.partial_bytes_acked = self.cwnd;
                    } else {
                        self.partial_bytes_acked -= self.cwnd;
                        self.cwnd += self.pmcds.min(1000);
                    }
                }
            }
            CongestionState::SlowStart => {
                if in_flight_before_sack >= self.cwnd {
                    self.cwnd += usize::max(2 * self.pmcds, bytes_acked);
                }
                if self.cwnd >= self.ssthresh {
                    self.cwnd = self.ssthresh;
                    self.change_state(CongestionState::CongestionAvoidance);
                }
            }
        }
    }

    pub fn enter_fast_recovery(&mut self) {
        self.ssthresh = self.cwnd / 2;
        self.cwnd = usize::max(
            self.ssthresh,
            usize::min(4 * self.pmcds, usize::max(2 * self.pmcds, 4404)),
        );
        self.change_state(CongestionState::FastRecovery);
    }

    pub fn rto_expired(&mut self) {
        self.ssthresh = self.cwnd / 2;
        self.cwnd = usize::min(4 * self.pmcds, usize::max(2 * self.pmcds, 4404));
        self.change_state(CongestionState::LossRecovery);
    }

    fn change_state(&mut self, state: CongestionState) {
        self.state = state;
    }

    pub fn state(&self) -> CongestionState {
        self.state
    }
}
