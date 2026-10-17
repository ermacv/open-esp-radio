//! Optional observations of the shared platform timer during an explicit window.
//! Counts include every task using that timer, not just the caller's futures.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    pub count: u32,
    pub total_micros: u64,
    pub maximum_micros: u32,
}
impl Timing {
    fn record(&mut self, elapsed: u64) -> Option<()> {
        let count = self.count.checked_add(1)?;
        let total = self.total_micros.checked_add(elapsed)?;
        let maximum = self.maximum_micros.max(u32::try_from(elapsed).ok()?);
        *self = Self {
            count,
            total_micros: total,
            maximum_micros: maximum,
        };
        Some(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub elapsed_micros: u64,
    pub invalid: bool,
    pub registrations: u32,
    pub due_at_registration: u32,
    pub due_at_program_start: u32,
    pub due_at_program_return: u32,
    pub irq_ack: Timing,
    pub programming: Timing,
    pub alarm_to_irq: Timing,
    pub deadline_lateness: Timing,
    pub irq_to_dispatch: Timing,
    pub dispatch: Timing,
    pub interrupts: u32,
    pub replaced: u32,
    pub stopped: u32,
    pub unmatched_interrupts: u32,
    /// IRQ entry overlaps the latest alarm programming; alarm latency is unattributable.
    pub overlapping_interrupts: u32,
    pub unmatched_dispatches: u32,
    pub coalesced_interrupts: u32,
    pub early_interrupts: u32,
    pub armed_at_end: bool,
    pub irq_pending_at_end: bool,
}

#[derive(Default)]
pub(crate) struct Recorder {
    started: Option<u64>,
    armed: Option<(u64, u64)>, // Requested absolute deadline, schedule() return.
    irq: Option<u64>,
    report: Report,
}

impl Recorder {
    pub(crate) const fn new() -> Self {
        Self {
            started: None,
            armed: None,
            irq: None,
            report: Report {
                elapsed_micros: 0,
                invalid: false,
                registrations: 0,
                due_at_registration: 0,
                due_at_program_start: 0,
                due_at_program_return: 0,
                irq_ack: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                programming: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                alarm_to_irq: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                deadline_lateness: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                irq_to_dispatch: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                dispatch: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                interrupts: 0,
                replaced: 0,
                stopped: 0,
                unmatched_interrupts: 0,
                overlapping_interrupts: 0,
                unmatched_dispatches: 0,
                coalesced_interrupts: 0,
                early_interrupts: 0,
                armed_at_end: false,
                irq_pending_at_end: false,
            },
        }
    }
    pub(crate) fn enabled(&self) -> bool {
        self.started.is_some()
    }
    pub(crate) fn begin(&mut self, now: u64) -> bool {
        if self.enabled() {
            return false;
        }
        *self = Self {
            started: Some(now),
            ..Self::new()
        };
        true
    }
    pub(crate) fn finish(&mut self, now: u64) -> Report {
        let Some(started) = self.started.take() else {
            return Report {
                invalid: true,
                ..Report::default()
            };
        };
        self.report.elapsed_micros = now.checked_sub(started).unwrap_or_else(|| {
            self.report.invalid = true;
            0
        });
        self.report.armed_at_end = self.armed.is_some();
        self.report.irq_pending_at_end = self.irq.is_some();
        self.report
    }
    pub(crate) fn registration(&mut self, deadline: u64, now: u64) {
        if !self.enabled() {
            return;
        }
        increment(&mut self.report.registrations, &mut self.report.invalid);
        if deadline <= now {
            increment(
                &mut self.report.due_at_registration,
                &mut self.report.invalid,
            );
        }
    }
    pub(crate) fn arm(&mut self, deadline: u64, start: u64, end: u64) {
        if !self.enabled() {
            return;
        }
        record(
            &mut self.report.programming,
            start,
            end,
            &mut self.report.invalid,
        );
        if deadline <= start {
            increment(
                &mut self.report.due_at_program_start,
                &mut self.report.invalid,
            );
        }
        if deadline <= end {
            increment(
                &mut self.report.due_at_program_return,
                &mut self.report.invalid,
            );
        }
        if self.armed.replace((deadline, end)).is_some() {
            increment(&mut self.report.replaced, &mut self.report.invalid);
        }
    }
    pub(crate) fn stop(&mut self) {
        if self.enabled() && self.armed.take().is_some() {
            increment(&mut self.report.stopped, &mut self.report.invalid);
        }
    }
    pub(crate) fn interrupt(&mut self, now: u64, acknowledged: u64) {
        if !self.enabled() {
            return;
        }
        record(
            &mut self.report.irq_ack,
            now,
            acknowledged,
            &mut self.report.invalid,
        );
        increment(&mut self.report.interrupts, &mut self.report.invalid);
        if let Some((deadline, programmed)) = self.armed.take() {
            if now < programmed && programmed <= acknowledged {
                // Entry is sampled before acquiring the timer mutex. Another core
                // can finish a replacement alarm before this IRQ acquires it.
                // Neither that alarm's deadline nor its latency belongs to a
                // provably matched IRQ. Retire it without fabricating an interval.
                increment(
                    &mut self.report.overlapping_interrupts,
                    &mut self.report.invalid,
                );
            } else {
                record(
                    &mut self.report.alarm_to_irq,
                    programmed,
                    now,
                    &mut self.report.invalid,
                );
                if now < deadline {
                    increment(&mut self.report.early_interrupts, &mut self.report.invalid);
                }
                self.report.invalid |= self
                    .report
                    .deadline_lateness
                    .record(now.saturating_sub(deadline))
                    .is_none();
            }
        } else {
            increment(
                &mut self.report.unmatched_interrupts,
                &mut self.report.invalid,
            );
        }
        if self.irq.is_some() {
            increment(
                &mut self.report.coalesced_interrupts,
                &mut self.report.invalid,
            );
        } else {
            self.irq = Some(now);
        }
    }
    pub(crate) fn dispatch(&mut self, start: u64, end: u64) {
        if !self.enabled() {
            return;
        }
        if let Some(irq) = self.irq.take() {
            record(
                &mut self.report.irq_to_dispatch,
                irq,
                start,
                &mut self.report.invalid,
            );
        } else {
            increment(
                &mut self.report.unmatched_dispatches,
                &mut self.report.invalid,
            );
        }
        record(
            &mut self.report.dispatch,
            start,
            end,
            &mut self.report.invalid,
        );
    }
}

fn increment(count: &mut u32, invalid: &mut bool) {
    match count.checked_add(1) {
        Some(next) => *count = next,
        None => *invalid = true,
    }
}
fn record(timing: &mut Timing, start: u64, end: u64, invalid: &mut bool) {
    *invalid |= end
        .checked_sub(start)
        .and_then(|elapsed| timing.record(elapsed))
        .is_none();
}

/// Exclusive diagnostic window. Drop stops recording, including cancellation.
/// It cannot move to another executor/core. Hardware timer ownership is unchanged.
#[cfg(feature = "chip")]
pub struct Window {
    active: bool,
    _local: core::marker::PhantomData<*mut ()>,
}
#[cfg(feature = "chip")]
impl Window {
    pub fn begin() -> Option<Self> {
        crate::time_driver::begin_observation().then(|| Self {
            active: true,
            _local: core::marker::PhantomData,
        })
    }
    pub fn finish(mut self) -> Report {
        self.active = false;
        crate::time_driver::finish_observation()
    }
}
#[cfg(feature = "chip")]
impl Drop for Window {
    fn drop(&mut self) {
        if self.active {
            let _ = crate::time_driver::finish_observation();
        }
    }
}

#[cfg(test)]
mod tests;
