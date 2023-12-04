use std::time::{Duration, Instant};

// https://datatracker.ietf.org/doc/rfc8899/
pub struct PmtuProbe {
    last_tested_size: usize,
    highest_successful: usize,
    last_success: bool,
    probe_in_flight: bool,
}

const PROBE_STEP_SIZE: usize = 1024 * 2;
impl PmtuProbe {
    pub fn new() -> Self {
        Self {
            last_tested_size: 1200,
            highest_successful: 1200,
            last_success: true,
            probe_in_flight: false,
        }
    }

    pub fn get_pmtu(&self) -> usize {
        self.highest_successful
    }

    pub fn probe_in_flight(&self) -> bool {
        self.probe_in_flight
    }

    pub fn next_probe_size(&mut self) -> usize {
        let probe_size = self.last_tested_size + PROBE_STEP_SIZE;
        self.last_tested_size = probe_size;
        self.probe_in_flight = true;
        probe_size
    }

    pub fn probe_success(&mut self, size: usize) {
        self.highest_successful = usize::max(self.highest_successful, size);
        self.last_success = true;
        self.probe_in_flight = false;
    }

    pub fn probe_timed_out(&mut self) {
        self.last_tested_size -= PROBE_STEP_SIZE + 128;
        self.last_success = false;
        self.probe_in_flight = false;
    }

    pub fn next_probe(&self, now: Instant) -> Instant {
        if self.last_success {
            now + Duration::from_millis(100)
        } else {
            now + Duration::from_millis(2000)
        }
    }
}
