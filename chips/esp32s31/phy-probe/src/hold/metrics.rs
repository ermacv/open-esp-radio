//! Diagnostic-only root-future poll and suspension observation.
//! Suspension is measured directly; it includes timers and scheduling latency.
use oer_diagnostics::Note;
#[derive(Clone, Copy, Default)]
pub struct Row {
    pub polls: u32,
    pub pending: u32,
    pub resumes: u32,
    pub suspended_us: u64,
    pub maximum_suspended_us: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Metrics {
    pub rows: [Row; 5],
    pub invalid: bool,
    pub poll_us: u64,
    pub maximum_poll_us: u64,
    active: u8,
    seen: u8,
    entered: Option<u64>,
    pending_since: Option<u64>,
}

impl Metrics {
    pub fn edge(&mut self, operation: usize, started: bool) {
        let Some(row) = self.rows.get_mut(operation) else {
            self.invalid = true;
            return;
        };
        // The admitted index is below the five rows, and therefore below u8::BITS.
        let mask = 1u8.wrapping_shl(operation as u32);
        if started {
            self.invalid |= self.active & mask != 0;
            self.active |= mask;
            if self.entered.is_some() && self.seen & mask == 0 {
                self.invalid |= add_u32(&mut row.polls, 1);
                self.seen |= mask;
            }
        } else {
            self.invalid |= self.active & mask == 0;
            self.active &= !mask;
        }
    }

    pub fn enter(&mut self, now: u64) {
        self.invalid |= self.entered.replace(now).is_some();
        self.seen = self.active;
        for (i, row) in self.rows.iter_mut().enumerate() {
            if self.active & 1u8.wrapping_shl(i as u32) != 0 {
                self.invalid |= add_u32(&mut row.polls, 1);
                if let Some(since) = self.pending_since {
                    if let Some(elapsed) = now.checked_sub(since) {
                        self.invalid |= add_u32(&mut row.resumes, 1);
                        self.invalid |= add_u64(&mut row.suspended_us, elapsed);
                        row.maximum_suspended_us = row.maximum_suspended_us.max(elapsed);
                    } else {
                        self.invalid = true;
                    }
                }
            }
        }
        self.pending_since = None;
    }

    pub fn exit(&mut self, now: u64, pending: bool) {
        match self.entered.take().and_then(|start| now.checked_sub(start)) {
            Some(elapsed) => {
                self.invalid |= add_u64(&mut self.poll_us, elapsed);
                self.maximum_poll_us = self.maximum_poll_us.max(elapsed);
            }
            None => self.invalid = true,
        }
        if pending {
            self.pending_since = Some(now);
            for (i, row) in self.rows.iter_mut().enumerate() {
                if self.active & 1u8.wrapping_shl(i as u32) != 0 {
                    self.invalid |= add_u32(&mut row.pending, 1);
                }
            }
        } else {
            self.invalid |= self.active != 0;
        }
    }
}

// Preserve the last representable total and invalidate the report on overflow,
// as HoldPolls::merge does for the engine's aggregate evidence.
fn add_u32(total: &mut u32, value: u32) -> bool {
    match total.checked_add(value) {
        Some(sum) => {
            *total = sum;
            false
        }
        None => true,
    }
}

fn add_u64(total: &mut u64, value: u64) -> bool {
    match total.checked_add(value) {
        Some(sum) => {
            *total = sum;
            false
        }
        None => true,
    }
}

/// UART is available only to the caller after the root future returns.
pub async fn report(tick: u8, metrics: &Metrics, output: &mut impl super::notes::Notes) {
    for (operation, row) in metrics.rows.iter().enumerate() {
        output
            .note(&Note::hold_poll(
                tick.into(),
                operation as u32,
                row.polls,
                row.pending,
                row.resumes,
            ))
            .await;
        output
            .note(&Note::hold_gap(
                tick.into(),
                operation as u32,
                row.suspended_us,
                row.maximum_suspended_us,
                metrics.invalid,
            ))
            .await;
    }
    output
        .note(&Note::hold_root(
            tick.into(),
            metrics.poll_us,
            metrics.maximum_poll_us,
        ))
        .await;
}

#[cfg(test)]
mod tests;
