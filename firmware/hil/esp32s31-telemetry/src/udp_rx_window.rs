//! Bounded UDP observation window. Silence is evidence, never an end condition.
//!
//! Time is supplied by the caller in monotonic microseconds. The first data
//! packet starts the window, with a bounded startup allowance for the host to
//! see SessionReady. If none arrives, the window starts when that allowance
//! expires. Terminal markers do not shorten the measurement; a separate grace
//! period admits markers but excludes late payload from throughput.

/// One bounded delivery milestone per session. Invalid payloads and control
/// markers cannot satisfy readiness for a maintenance workload.
#[derive(Default)]
pub struct DeliveryStart {
    valid: u16,
}

impl DeliveryStart {
    pub fn observe(&mut self, length_matches: bool, sequence: Option<i32>) -> bool {
        if self.valid == 256 || !length_matches || !sequence.is_some_and(|n| n >= 0) {
            return false;
        }
        self.valid += 1;
        self.valid == 256
    }
}

pub struct RxWindow {
    armed: u64,
    start: u64,
    duration: u64,
    grace: u64,
    silence_threshold: u64,
    first: Option<u64>,
    last: Option<u64>,
    pauses: u32,
    maximum_silence: u64,
    maximum_silence_start: u64,
}

impl RxWindow {
    pub fn new(armed: u64, duration: u64, startup: u64, grace: u64, silence: u64) -> Self {
        Self {
            armed,
            start: armed.saturating_add(startup),
            duration,
            grace,
            silence_threshold: silence,
            first: None,
            last: None,
            pauses: 0,
            maximum_silence: 0,
            maximum_silence_start: 0,
        }
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.duration)
    }

    pub fn deadline(&self) -> u64 {
        self.end().saturating_add(self.grace)
    }

    pub fn finished(&self, now: u64, terminal: bool) -> bool {
        now >= self.deadline() || (now >= self.end() && terminal)
    }

    pub fn next_deadline(&self, now: u64) -> u64 {
        if now < self.end() {
            self.end()
        } else {
            self.deadline()
        }
    }

    /// Returns false for payload outside the measurement window.
    pub fn data(&mut self, now: u64) -> bool {
        if now >= self.end() {
            return false;
        }
        if self.first.is_none() {
            self.start = self.start.min(now);
            self.first = Some(now);
        }
        let gap = now.saturating_sub(self.last.unwrap_or(self.start));
        self.observe_gap(gap, self.last.unwrap_or(self.start));
        self.last = Some(now);
        true
    }

    fn observe_gap(&mut self, gap: u64, start: u64) {
        if gap > self.maximum_silence {
            self.maximum_silence = gap;
            self.maximum_silence_start = start.saturating_sub(self.start);
        }
        if gap >= self.silence_threshold {
            self.pauses = self.pauses.saturating_add(1);
        }
    }

    pub fn summary(&self) -> SilenceSummary {
        let tail = self.end().saturating_sub(self.last.unwrap_or(self.start));
        SilenceSummary {
            first_delay_micros: self.first.map(|first| first.saturating_sub(self.armed)),
            pauses: self
                .pauses
                .saturating_add(u32::from(tail >= self.silence_threshold)),
            maximum_silence_micros: self.maximum_silence.max(tail),
            maximum_silence_start_micros: if tail > self.maximum_silence {
                self.last.unwrap_or(self.start).saturating_sub(self.start)
            } else {
                self.maximum_silence_start
            },
            trailing_silence_micros: tail,
        }
    }
}

pub struct SilenceSummary {
    pub first_delay_micros: Option<u64>,
    pub pauses: u32,
    pub maximum_silence_micros: u64,
    pub maximum_silence_start_micros: u64,
    pub trailing_silence_micros: u64,
}

#[cfg(test)]
mod tests;
