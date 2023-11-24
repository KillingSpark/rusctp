use crate::packet::Tsn;

#[derive(PartialEq, Eq)]
enum CongestionState {
    SlowStart,
    FastRecovery,
    CongestionAvoidance,
}

pub struct PerDestinationInfo {
    state: CongestionState,
    pub pmcds: usize,
    cwnd: usize,
    ssthresh: usize,
    _partial_bytes_acked: usize,
    bytes_acked_counter: usize,
    bytes_acked_start_tsn: Option<Tsn>,
}

impl PerDestinationInfo {
    pub fn new(pmtu: usize) -> Self {
        let pmcds = pmtu - 12;
        Self {
            state: CongestionState::SlowStart,
            pmcds,
            cwnd: usize::min(4 * pmcds, usize::max(2 * pmcds, 4404)),
            ssthresh: usize::MAX,
            _partial_bytes_acked: 0,
            bytes_acked_counter: 0,
            bytes_acked_start_tsn: None,
        }
    }

    pub fn send_limit(&self, current_outstanding: usize) -> usize {
        self.cwnd.saturating_sub(current_outstanding)
    }

    pub fn bytes_acked(&mut self, bytes_acked: usize, up_to_tsn: Tsn) {
        self.bytes_acked_counter += bytes_acked;
        if bytes_acked > 0 && self.state == CongestionState::FastRecovery {
            // Fast recovery did move the tsn, go back to slowstart
            self.change_state(CongestionState::SlowStart);
        }
        if let Some(bytes_acked_start_tsn) = { self.bytes_acked_start_tsn } {
            if up_to_tsn >= bytes_acked_start_tsn {
                self.adjust_cwnd();
            }
        }
    }

    pub fn enter_fast_recovery(&mut self) {
        self.ssthresh = self.cwnd / 2;
        self.cwnd = self.ssthresh;
        self.change_state(CongestionState::FastRecovery);
    }

    pub fn rto_expired(&mut self) {
        self.ssthresh = self.cwnd / 2;
        self.cwnd = usize::min(4 * self.pmcds, usize::max(2 * self.pmcds, 4404));
        self.change_state(CongestionState::CongestionAvoidance);
    }

    fn change_state(&mut self, state: CongestionState) {
        self.bytes_acked_start_tsn = None;
        self.bytes_acked_counter = 0;
        self.state = state;
    }

    pub fn tsn_sent(&mut self, tsn: Tsn) {
        if self.bytes_acked_start_tsn.is_none() {
            self.bytes_acked_start_tsn = Some(tsn);
            self.bytes_acked_counter = 0;
        }
    }

    fn adjust_cwnd(&mut self) {
        let bytes_acked = self.bytes_acked_counter;
        self.bytes_acked_counter = 0;
        self.bytes_acked_start_tsn = None;
        match self.state {
            CongestionState::SlowStart => {
                if bytes_acked >= self.cwnd {
                    self.cwnd += usize::max(bytes_acked, usize::min(self.pmcds, bytes_acked));
                    if self.cwnd >= self.ssthresh {
                        self.cwnd = self.ssthresh;
                        self.state = CongestionState::CongestionAvoidance;
                    }
                }
            }
            CongestionState::CongestionAvoidance => {
                if bytes_acked >= self.cwnd {
                    self.cwnd += usize::min(self.pmcds, bytes_acked);
                }
            }
            CongestionState::FastRecovery => {
                // TODO do we do anything here?
            }
        }
    }
}
