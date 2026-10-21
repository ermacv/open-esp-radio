//! Image-local coarse boundaries; no clock reads for inner operation callbacks.
use oer_hil_protocol::phy::{HoldInterval, HoldOperation, HoldSample};

pub struct Recorder {
    sample: HoldSample,
    active: [Option<u64>; 5],
    last: u64,
}
impl Recorder {
    pub fn new(tick: u8) -> Self {
        Self {
            sample: HoldSample {
                tick,
                committed: 0,
                invalid: false,
                intervals: heapless::Vec::new(),
            },
            active: [None; 5],
            // Zero is the lower bound of every input timestamp, so the
            // initial comparison accepts every first sample without an Option.
            last: 0,
        }
    }
    pub fn observe(&mut self, operation: HoldOperation, started: bool, failed: bool, now: u64) {
        if now < self.last {
            self.sample.invalid = true;
        }
        self.last = now;
        let slot = &mut self.active[operation as usize];
        if started {
            if slot.is_some() {
                self.sample.invalid = true;
            } else {
                *slot = Some(now);
            }
        } else if let Some(begin) = slot.take() {
            let interval = HoldInterval {
                operation,
                started_micros: begin,
                finished_micros: now,
                failed,
            };
            let overflow = self.sample.intervals.push(interval).is_err();
            if failed || now < begin || overflow {
                self.sample.invalid = true;
            }
        } else {
            self.sample.invalid = true;
        }
    }
    pub fn finish(mut self, committed: u8, failed: bool) -> HoldSample {
        self.sample.committed = committed;
        self.sample.invalid |= failed || self.active.iter().any(Option::is_some);
        self.sample
    }
}
