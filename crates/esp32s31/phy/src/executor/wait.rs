//! Hardware wait reasons, the PHY's waits on the image's monotonic time and
//! scoped runtime calibration delay evidence.
//!
//! A PHY wait is either a short minimum settle, which completes inside the
//! exclusive hardware transaction through a [`PhyShortDelay`], or a wait on
//! the caller's [`oer_time::Timer`]: scheduling backoff, long settles and
//! readiness retries. [`delay`] chooses between them; [`delay_observed`] also
//! reports the wait against the same start and deadline.

use core::{future::Future, pin::pin, task::Poll};

use oer_time::{Duration, Timer};

pub mod tx;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod poll;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Executor backoff before retrying hardware-bus admission.
    /// This classification does not establish a vendor-required interval.
    BusBusy,
    /// Executor backoff before the next completion/readiness observation.
    /// A completion predicate alone does not require a delay before sampling.
    Completion,
    /// Minimum settling interval after a hardware state change.
    Settle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Dcode,
    Rfpll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Started {
        requested_micros: u64,
    },
    Completed {
        elapsed_micros: u64,
        lateness_micros: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    pub count: u16,
    pub requested_micros: u32,
    pub elapsed_micros: u32,
    pub maximum_lateness_micros: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bus {
    /// Subset of waits caused by BusyAtStart; the rest precede completion reads.
    pub bus_busy: u16,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub i2c: Bus,
    pub rfpll_i2c: Bus,
    /// Explicit RFPLL settling/lock delays, separate from its I2C transactions.
    pub rfpll_settle: Timing,
    /// Existing analog RFPLL_LOCK_STATUS samples, decoded without extra reads.
    pub pll_locked: u16,
    pub pll_unlocked: u16,
}

#[derive(Default)]
pub(crate) struct Recorder {
    report: Report,
    active: Option<(Scope, Kind, u64)>,
    invalid: bool,
}

impl Recorder {
    pub(crate) fn observe(&mut self, scope: Scope, kind: Kind, event: Event) {
        match event {
            Event::Started { requested_micros } => {
                if self.active.is_some() || (scope == Scope::Dcode && kind == Kind::Settle) {
                    self.invalid = true;
                    return;
                }
                self.active = Some((scope, kind, requested_micros));
            }
            Event::Completed {
                elapsed_micros,
                lateness_micros,
            } => {
                let Some((started_scope, started_kind, requested)) = self.active.take() else {
                    self.invalid = true;
                    return;
                };
                if (started_scope, started_kind) != (scope, kind)
                    || elapsed_micros.checked_sub(requested) != Some(lateness_micros)
                {
                    self.invalid = true;
                    return;
                }
                let (Ok(requested), Ok(elapsed), Ok(lateness)) = (
                    u32::try_from(requested),
                    u32::try_from(elapsed_micros),
                    u32::try_from(lateness_micros),
                ) else {
                    self.invalid = true;
                    return;
                };
                let timing = match kind {
                    Kind::Settle => &mut self.report.rfpll_settle,
                    Kind::BusBusy | Kind::Completion => {
                        let i2c = match scope {
                            Scope::Dcode => &mut self.report.i2c,
                            Scope::Rfpll => &mut self.report.rfpll_i2c,
                        };
                        if kind == Kind::BusBusy {
                            let Some(next) = i2c.bus_busy.checked_add(1) else {
                                self.invalid = true;
                                return;
                            };
                            i2c.bus_busy = next;
                        }
                        &mut i2c.timing
                    }
                };
                let (Some(count), Some(total_requested), Some(total_elapsed)) = (
                    timing.count.checked_add(1),
                    timing.requested_micros.checked_add(requested),
                    timing.elapsed_micros.checked_add(elapsed),
                ) else {
                    self.invalid = true;
                    return;
                };
                *timing = Timing {
                    count,
                    requested_micros: total_requested,
                    elapsed_micros: total_elapsed,
                    maximum_lateness_micros: timing.maximum_lateness_micros.max(lateness),
                };
            }
        }
    }

    pub(crate) fn pll_lock(&mut self, locked: bool) {
        let count = if locked {
            &mut self.report.pll_locked
        } else {
            &mut self.report.pll_unlocked
        };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.invalid = true,
        }
    }

    pub(crate) fn report(&self) -> Report {
        self.report
    }
    pub(crate) fn is_complete(&self) -> bool {
        !self.invalid && self.active.is_none()
    }
}

/// Blocking settle used inside one already-admitted PHY hardware transaction.
///
/// Short analog settles are part of the transaction itself. They must not arm
/// an executor timer or return `Pending`: the radio cannot do useful work in
/// the interval and the vendor implementation uses the same blocking model.
pub trait PhyShortDelay {
    /// Largest minimum settle that the backend can complete synchronously.
    const MAX_MICROS: u32;

    /// Complete a short minimum settle without constructing a future.
    /// False means that the requested interval is outside the implementation's
    /// proven blocking range.
    fn settle_micros(micros: u32) -> bool;
}

/// Whether a wait of `kind` and `micros` is a short settle `S` completes
/// synchronously. Readiness retries keep their timer cadence at any size.
const fn synchronous_settle<S: PhyShortDelay>(kind: Kind, micros: u64) -> bool {
    matches!(kind, Kind::Settle) && micros != 0 && micros <= S::MAX_MICROS as u64
}

/// Wait at least `micros` for `kind`.
///
/// A short settle completes at its first poll through `S`; every other wait
/// ends on `timer` at the deadline taken when this is called, so the time
/// before the first poll counts. A deadline past the timer's range is never
/// reached: the wait fail-stops instead of ending early.
pub fn delay<S: PhyShortDelay, T: Timer + ?Sized>(
    timer: &T,
    kind: Kind,
    micros: u64,
) -> impl Future<Output = ()> + '_ {
    let deadline = (!synchronous_settle::<S>(kind, micros))
        .then(|| timer.now().checked_add(Duration::from_micros(micros)));
    async move {
        match deadline {
            None => {
                let micros = u32::try_from(micros).expect("a short settle fits u32");
                assert!(
                    S::settle_micros(micros),
                    "a short settle is within its bound"
                );
            }
            Some(Some(deadline)) => timer.wait_until(deadline).await,
            Some(None) => core::future::pending().await,
        }
    }
}

/// [`delay`], reporting the wait to `observe` when `enabled`: its start at
/// the first poll and, on completion, the time elapsed since this was called
/// and the lateness past `micros`, measured on `timer`. The completion is
/// observed after the wait ends, so observer work cannot shorten it.
pub fn delay_observed<'t, S: PhyShortDelay, T: Timer + ?Sized>(
    timer: &'t T,
    kind: Kind,
    micros: u64,
    enabled: bool,
    mut observe: impl FnMut(Event) + 't,
) -> impl Future<Output = ()> + 't {
    let start = timer.now();
    async move {
        let mut wait = pin!(delay::<S, T>(timer, kind, micros));
        let mut started = false;
        core::future::poll_fn(|context| {
            let result = wait.as_mut().poll(context);
            if enabled {
                if !started {
                    observe(Event::Started {
                        requested_micros: micros,
                    });
                    started = true;
                }
                if result.is_ready() {
                    let elapsed_micros = timer.now().saturating_duration_since(start).as_micros();
                    observe(Event::Completed {
                        elapsed_micros,
                        lateness_micros: elapsed_micros.saturating_sub(micros),
                    });
                }
            }
            match result {
                Poll::Ready(()) => Poll::Ready(()),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests;
