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
        let mask = 1 << operation;
        if started {
            self.invalid |= self.active & mask != 0;
            self.active |= mask;
            if self.entered.is_some() && self.seen & mask == 0 {
                self.rows[operation].polls += 1;
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
            if self.active & (1 << i) != 0 {
                row.polls += 1;
                if let Some(since) = self.pending_since {
                    if let Some(elapsed) = now.checked_sub(since) {
                        row.resumes += 1;
                        row.suspended_us += elapsed;
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
                self.poll_us += elapsed;
                self.maximum_poll_us = self.maximum_poll_us.max(elapsed);
            }
            None => self.invalid = true,
        }
        if pending {
            self.pending_since = Some(now);
            for (i, row) in self.rows.iter_mut().enumerate() {
                if self.active & (1 << i) != 0 {
                    row.pending += 1;
                }
            }
        } else {
            self.invalid |= self.active != 0;
        }
    }
}

/// Keep root instrumentation identical for forced and natural policy.
pub async fn observe<F: core::future::Future, C: oer_time::Clock>(
    future: F,
    clock: &C,
    metrics: &core::cell::RefCell<Metrics>,
) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|cx| {
        metrics.borrow_mut().enter(clock.now().as_micros());
        let result = future.as_mut().poll(cx);
        metrics
            .borrow_mut()
            .exit(clock.now().as_micros(), result.is_pending());
        result
    })
    .await
}

/// UART is available only to the caller after the root future returns.
pub async fn report(tick: u8, metrics: &Metrics, output: &mut impl super::Notes) {
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
