//! One light sleep of the HP system until an LP-timer target.
//!
//! [`sleep_until`] writes the PMU's sleep state for a sleep that powers no
//! domain down, arms the LP main timer's first target as the only wake
//! source, requests sleep and reads the outcome. The PMU stalls the CPU
//! inside the outcome poll; the poll is the sleep itself, the idle path's
//! counterpart of a wait for interrupt, and the LP timer bounds it: no
//! outcome by the request's bound is [`SleepError::NoOutcome`].
//!
//! The caller masks interrupts around the call: a pending interrupt does not
//! wake the HP system, since only the LP timer is a wake source, and no
//! handler runs between the request and the outcome.

use crate::Access;

#[cfg(test)]
mod tests;

/// A reading or target of the LP main timer, in slow-clock cycles.
///
/// The counter is [`RtcTicks::BITS`] wide; no value beyond it exists.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RtcTicks(u64);

impl RtcTicks {
    /// The counter's width.
    pub const BITS: u32 = 48;
    /// The largest count.
    pub const MAX: u64 = (1 << Self::BITS) - 1;

    /// The count `ticks`, if the counter holds it.
    pub const fn new(ticks: u64) -> Option<Self> {
        if ticks <= Self::MAX {
            Some(Self(ticks))
        } else {
            None
        }
    }

    /// The count.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The count `ticks` later, if the counter holds it.
    pub const fn checked_add(self, ticks: u64) -> Option<Self> {
        match self.0.checked_add(ticks) {
            Some(sum) => Self::new(sum),
            None => None,
        }
    }

    /// The cycles since `earlier`, zero if `earlier` is later.
    pub const fn saturating_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// The HP system's functional and bus clock gates while it sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepClocks {
    /// Every gate the sleep state controls stays open: only the CPU stalls.
    Running,
    /// Every gate the sleep state controls closes.
    Gated,
}

/// One sleep: when it ends and how long the outcome may take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SleepRequest {
    /// The LP-timer count that wakes the HP system.
    pub wake_at: RtcTicks,
    /// The count after which a missing outcome is
    /// [`SleepError::NoOutcome`]; it lies after `wake_at`.
    pub give_up_after: RtcTicks,
    /// The clock gates while the HP system sleeps.
    pub clocks: SleepClocks,
}

/// What ended the sleep request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The HP system slept and a wake source woke it; `causes` is the PMU's
    /// wake-cause word.
    Woke {
        /// The PMU's wake-cause word.
        causes: u32,
    },
    /// A reject source refused the request before the HP system slept;
    /// `causes` is the PMU's reject-cause word.
    Rejected {
        /// The PMU's reject-cause word.
        causes: u32,
    },
}

/// A sleep request that ended with an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slept {
    /// What ended it.
    pub outcome: Outcome,
    /// The LP-timer count just before the request.
    pub requested: RtcTicks,
    /// The LP-timer count just after the outcome.
    pub resumed: RtcTicks,
}

/// Why a sleep request ended without an outcome; the wake source is
/// disabled and the status acknowledged again on every error after the
/// request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepError {
    /// The request's bound does not lie after its target; nothing was
    /// written.
    Bound,
    /// The target lies within the minimum sleep of the current count;
    /// nothing was written.
    TooSoon {
        /// The count when the request was checked.
        now: RtcTicks,
        /// The request's target.
        wake_at: RtcTicks,
    },
    /// A counter reading is wider than the counter.
    Counter(u64),
    /// The PMU reported neither a wake nor a reject by the request's bound.
    NoOutcome {
        /// The count that passed the bound.
        now: RtcTicks,
    },
}

/// The PMU's raw sleep outcome, read without acknowledgement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RawOutcome {
    /// A wake source woke the HP system.
    pub woke: bool,
    /// A reject source refused the request.
    pub rejected: bool,
}

/// Outcome polls between two readings of the LP timer.
///
/// The PMU stalls the CPU within a few polls of the request; the timer is
/// read only while it has not, so the poll does not keep the LP bus busy
/// while the PMU enters sleep.
pub const POLLS_PER_TIMER_READ: u32 = 256;

/// The PMU's sleep request and the LP main timer, one register operation
/// per method.
pub trait Registers {
    /// The fewest slow-clock cycles from the check of the current count to
    /// the target: the minimum sleep the waits configure and the entry's
    /// own time.
    const MIN_SLEEP: u64;

    /// Write the sleep state: no domain powered down, the PLLs, the main
    /// crystal and the fast RC oscillator kept on and the regulators at
    /// their active output; `clocks` selects the clock gates.
    fn configure_state(&mut self, access: Access<'_>, clocks: SleepClocks);
    /// Write the sleep's waits: the minimum sleep and the analog, power,
    /// crystal and PLL waits.
    fn configure_waits(&mut self, access: Access<'_>);
    /// Capture the LP main timer's counter.
    fn capture_rtc(&mut self, access: Access<'_>);
    /// The counter the last capture took.
    fn captured_rtc(&self, access: Access<'_>) -> u64;
    /// Acknowledge the first target's wake status.
    fn acknowledge_rtc_wake(&mut self, access: Access<'_>);
    /// Write the first target and enable it.
    fn arm_rtc_target(&mut self, access: Access<'_>, at: RtcTicks);
    /// Make the LP timer the only wake source, or disable every source.
    fn select_wake(&mut self, access: Access<'_>, rtc_timer: bool);
    /// Disable sleep rejection.
    fn disable_reject(&mut self, access: Access<'_>);
    /// Acknowledge the raw wake and reject status and the reject cause.
    fn acknowledge_outcome(&mut self, access: Access<'_>);
    /// Request sleep.
    fn request_sleep(&mut self, access: Access<'_>);
    /// The raw wake and reject status.
    fn outcome(&mut self, access: Access<'_>) -> RawOutcome;
    /// The PMU's wake-cause word.
    fn wake_causes(&self, access: Access<'_>) -> u32;
    /// The PMU's reject-cause word.
    fn reject_causes(&self, access: Access<'_>) -> u32;
}

/// The LP timer's current count.
pub fn now<R: Registers>(registers: &mut R) -> Result<RtcTicks, SleepError> {
    registers.capture_rtc(Access::new());
    let ticks = registers.captured_rtc(Access::new());
    RtcTicks::new(ticks).ok_or(SleepError::Counter(ticks))
}

/// Sleep until the LP timer reaches `request.wake_at`, or until a reject.
///
/// A reject is an [`Outcome`], not an error: the HP system did not sleep
/// and the caller may retry. When the HP system both woke and was refused,
/// the reject is reported, as the vendor's sleep finish reports it.
pub fn sleep_until<R: Registers>(
    registers: &mut R,
    request: SleepRequest,
) -> Result<Slept, SleepError> {
    if request.give_up_after <= request.wake_at {
        return Err(SleepError::Bound);
    }
    let requested = now(registers)?;
    let too_soon = SleepError::TooSoon {
        now: requested,
        wake_at: request.wake_at,
    };
    let earliest = requested.checked_add(R::MIN_SLEEP).ok_or(too_soon)?;
    if request.wake_at <= earliest {
        return Err(too_soon);
    }
    registers.configure_state(Access::new(), request.clocks);
    registers.configure_waits(Access::new());
    registers.disable_reject(Access::new());
    registers.acknowledge_outcome(Access::new());
    registers.acknowledge_rtc_wake(Access::new());
    registers.arm_rtc_target(Access::new(), request.wake_at);
    registers.select_wake(Access::new(), true);
    registers.request_sleep(Access::new());
    let outcome = poll(registers, request.give_up_after);
    let resumed = now(registers);
    registers.select_wake(Access::new(), false);
    registers.acknowledge_outcome(Access::new());
    registers.acknowledge_rtc_wake(Access::new());
    let outcome = outcome?;
    Ok(Slept {
        outcome,
        requested,
        resumed: resumed?,
    })
}

/// Poll the outcome; read the LP timer every [`POLLS_PER_TIMER_READ`]
/// polls and give up once it passes `give_up_after`.
fn poll<R: Registers>(registers: &mut R, give_up_after: RtcTicks) -> Result<Outcome, SleepError> {
    let mut polls: u32 = 0;
    loop {
        let raw = registers.outcome(Access::new());
        if raw.rejected {
            return Ok(Outcome::Rejected {
                causes: registers.reject_causes(Access::new()),
            });
        }
        if raw.woke {
            return Ok(Outcome::Woke {
                causes: registers.wake_causes(Access::new()),
            });
        }
        polls = polls.wrapping_add(1);
        if polls.is_multiple_of(POLLS_PER_TIMER_READ) {
            let current = now(registers)?;
            if current > give_up_after {
                return Err(SleepError::NoOutcome { now: current });
            }
        }
    }
}
