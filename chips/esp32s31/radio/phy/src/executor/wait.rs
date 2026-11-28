//! Hardware wait reasons, the PHY's waits on the image's monotonic time and
//! scoped runtime calibration delay evidence.
//!
//! A PHY wait is either a short minimum settle, which completes inside the
//! exclusive hardware transaction through a [`PhyShortDelay`], or a wait on
//! the caller's [`oer_portable_clock::Timer`]: scheduling backoff, long settles and
//! readiness retries. [`delay`] chooses between them; [`delay_observed`] also
//! reports the wait against the same start and deadline.

use core::{future::Future, pin::pin, task::Poll};

use oer_portable_clock::Timer;
use oer_portable_settle::{SettleCounter, SettleLimit};
use oer_portable_time::Duration;

pub mod tx;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod i2c;

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
    /// The wait did not run its length.
    Refused,
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
            Event::Refused => {
                self.active = None;
                self.invalid = true;
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

    /// Complete a short minimum settle of `limit` on `counter`, without
    /// constructing a future.
    fn settle<K: SettleCounter + ?Sized>(counter: &K, limit: SettleLimit);
}

/// What a counted wait was, by how it ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waited {
    /// A [`Kind::Settle`] short enough to complete synchronously: spun.
    SettleSpun,
    /// A [`Kind::Settle`] longer than a short settle: on the timer.
    SettleTimed,
    /// A [`Kind::Completion`] wait on the timer.
    Completion,
    /// A [`Kind::BusBusy`] wait on the timer.
    BusBusy,
    /// A step engine's wait before its next observation, on the timer.
    EngineRetry,
    /// A cooperative yield to the executor: nothing is asked, its time is
    /// until the engine is polled again.
    Yield,
}

/// Where a timer's waits are counted: each wait's kind, the time it asked
/// and the time it took, both in microseconds.
pub trait WaitTally {
    /// Whether this tally counts: one that does not reads no clock.
    const COUNTS: bool;
    fn waited(&self, kind: Waited, asked_micros: u64, taken_micros: u64);
}

/// A tally that counts nothing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Uncounted;

impl WaitTally for Uncounted {
    const COUNTS: bool = false;
    fn waited(&self, _kind: Waited, _asked_micros: u64, _taken_micros: u64) {}
}

impl<W: WaitTally + ?Sized> WaitTally for &W {
    const COUNTS: bool = W::COUNTS;
    fn waited(&self, kind: Waited, asked_micros: u64, taken_micros: u64) {
        (**self).waited(kind, asked_micros, taken_micros);
    }
}

/// The PHY's timer beside its settle counter: what an executor gives the
/// PHY's operations, so that a short settle counts cycles at the adopted CPU
/// rate and every other wait ends on the timer.
///
/// `W` is where its waits are counted: [`Uncounted`] unless an image asks
/// what its operations waited for ([`PhyTimer::counted`]).
#[derive(Clone, Copy, Debug)]
pub struct PhyTimer<'t, T: ?Sized, K, W = Uncounted> {
    timer: &'t T,
    counter: K,
    tally: W,
}

impl<'t, T: ?Sized, K> PhyTimer<'t, T, K> {
    pub const fn new(timer: &'t T, counter: K) -> Self {
        Self {
            timer,
            counter,
            tally: Uncounted,
        }
    }
}

impl<'t, T: ?Sized, K, W: WaitTally> PhyTimer<'t, T, K, W> {
    /// The timer whose every wait is counted in `tally`.
    pub const fn counted(timer: &'t T, counter: K, tally: W) -> Self {
        Self {
            timer,
            counter,
            tally,
        }
    }
}

impl<T: ?Sized, K, W> PhyTimer<'_, T, K, W> {
    /// Where this timer's waits are counted.
    pub const fn tally(&self) -> &W {
        &self.tally
    }
}

impl<T: ?Sized, K, W: WaitTally> WaitTally for PhyTimer<'_, T, K, W> {
    const COUNTS: bool = W::COUNTS;
    fn waited(&self, kind: Waited, asked_micros: u64, taken_micros: u64) {
        self.tally.waited(kind, asked_micros, taken_micros);
    }
}

impl<T: oer_portable_clock::Clock + ?Sized, K, W> oer_portable_clock::Clock
    for PhyTimer<'_, T, K, W>
{
    fn now(&self) -> oer_portable_time::Instant {
        self.timer.now()
    }
}

impl<T: Timer + ?Sized, K, W> Timer for PhyTimer<'_, T, K, W> {
    fn wait_until(&self, deadline: oer_portable_time::Instant) -> impl Future<Output = ()> {
        self.timer.wait_until(deadline)
    }
}

impl<T: oer_portable_cooperate::CooperativeTimer + ?Sized, K, W>
    oer_portable_cooperate::CooperativeTimer for PhyTimer<'_, T, K, W>
{
    fn should_yield(&self) -> bool {
        self.timer.should_yield()
    }
}

impl<T: ?Sized, K: SettleCounter, W> SettleCounter for PhyTimer<'_, T, K, W> {
    fn cycles(&self, limit: SettleLimit) -> u32 {
        self.counter.cycles(limit)
    }
    fn sample(&self) -> u32 {
        self.counter.sample()
    }
}

/// The vendor's one-microsecond settle.
pub(crate) const SETTLE_ONE_MICRO: SettleLimit = match SettleLimit::new(Duration::from_micros(1)) {
    Some(limit) => limit,
    None => panic!("one microsecond lies within the longest a settle may wait"),
};

/// The vendor's two-microsecond settle.
pub(crate) const SETTLE_TWO_MICROS: SettleLimit = match SettleLimit::new(Duration::from_micros(2)) {
    Some(limit) => limit,
    None => panic!("two microseconds lie within the longest a settle may wait"),
};

/// The settle of a table's `micros`, if `S` completes it synchronously: not
/// zero and at most [`PhyShortDelay::MAX_MICROS`].
pub(crate) const fn short_limit<S: PhyShortDelay>(micros: u64) -> Option<SettleLimit> {
    if micros == 0 || micros > S::MAX_MICROS as u64 {
        None
    } else {
        SettleLimit::new(Duration::from_micros(micros))
    }
}

/// The settle of a wait of `kind` and `micros`, if it is a short settle `S`
/// completes synchronously. Readiness retries keep their timer cadence at any
/// size.
const fn synchronous_settle<S: PhyShortDelay>(kind: Kind, micros: u64) -> Option<SettleLimit> {
    if matches!(kind, Kind::Settle) {
        short_limit::<S>(micros)
    } else {
        None
    }
}

/// Why a wait could not run its full length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWaitRefused {
    /// The deadline is past the timer's range.
    Deadline,
}

/// Wait at least `micros` for `kind`.
///
/// A short settle completes at its first poll through `S`, counted on
/// `timer`'s counter; every other wait ends on `timer` at the deadline taken
/// when this is called, so the time before the first poll counts.
///
/// # Errors
///
/// The wait did not run its length; the caller must not act as if it had.
pub fn delay<S: PhyShortDelay, T: Timer + SettleCounter + WaitTally + ?Sized>(
    timer: &T,
    kind: Kind,
    micros: u64,
) -> impl Future<Output = Result<(), PhyWaitRefused>> + '_ {
    let settle = synchronous_settle::<S>(kind, micros);
    // A spun settle reads the clock only for a tally that counts.
    let start = (T::COUNTS || settle.is_none()).then(|| timer.now());
    let deadline = match (settle, start) {
        (None, Some(start)) => Some(start.checked_add(Duration::from_micros(micros))),
        _ => None,
    };
    async move {
        let waited = match (settle, deadline) {
            (Some(limit), _) => {
                S::settle(timer, limit);
                Waited::SettleSpun
            }
            (None, Some(Some(deadline))) => {
                timer.wait_until(deadline).await;
                match kind {
                    Kind::Settle => Waited::SettleTimed,
                    Kind::Completion => Waited::Completion,
                    Kind::BusBusy => Waited::BusBusy,
                }
            }
            (None, _) => return Err(PhyWaitRefused::Deadline),
        };
        if let (true, Some(start)) = (T::COUNTS, start) {
            let taken = timer.now().saturating_duration_since(start).as_micros();
            timer.waited(waited, micros, taken);
        }
        Ok(())
    }
}

/// [`delay`], reporting the wait to `observe` when `enabled`: its start at
/// the first poll and, on completion, the time elapsed since this was called
/// and the lateness past `micros`, measured on `timer`. The completion is
/// observed after the wait ends, so observer work cannot shorten it.
pub fn delay_observed<'t, S: PhyShortDelay, T: Timer + SettleCounter + WaitTally + ?Sized>(
    timer: &'t T,
    kind: Kind,
    micros: u64,
    enabled: bool,
    mut observe: impl FnMut(Event) + 't,
) -> impl Future<Output = Result<(), PhyWaitRefused>> + 't {
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
                match result {
                    Poll::Ready(Ok(())) => {
                        let elapsed_micros =
                            timer.now().saturating_duration_since(start).as_micros();
                        observe(Event::Completed {
                            elapsed_micros,
                            lateness_micros: elapsed_micros.saturating_sub(micros),
                        });
                    }
                    Poll::Ready(Err(_)) => observe(Event::Refused),
                    Poll::Pending => {}
                }
            }
            result
        })
        .await
    }
}

#[cfg(test)]
mod tests;
