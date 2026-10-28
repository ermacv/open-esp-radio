//! Named boot waits under one boot deadline.
//!
//! The bootstrap runs before any executor: it polls the hardware. Each poll
//! is a [`BootWait`], a name and a bound the chip takes from its source, and
//! every wait runs under one [`BootDeadline`] whose [`BootBudget`] lies below
//! the RTC watchdog the bootloader armed. A wait ends with its value, or with
//! a [`WaitFailure`] naming it: past its own bound, or past the deadline.
//!
//! Like [`oer_time::settle`], a poll reads the time before each attempt and
//! runs one attempt after the limit was seen, so an interrupt between an
//! attempt and the time check never fails a wait whose hardware completed
//! meanwhile.

use oer_time::{Clock, Duration, Instant};

/// One named boot wait: what it waits for and the longest it may take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootWait {
    name: &'static str,
    bound: Duration,
}

impl BootWait {
    /// A wait named `name` (the `wait=` word of a failure line) bounded by
    /// `bound`. A zero bound is refused with the wait's name.
    pub const fn new(name: &'static str, bound: Duration) -> Result<Self, WaitFailure> {
        if bound.as_micros() == 0 {
            return Err(WaitFailure::ZeroBound(name));
        }
        Ok(Self { name, bound })
    }

    /// The wait's name.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The longest the wait may take.
    pub const fn bound(self) -> Duration {
        self.bound
    }
}

/// The time the whole bring-up may take, below the armed RTC watchdog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootBudget(Duration);

impl BootBudget {
    /// A budget of `budget`, or `None` unless it is nonzero and shorter than
    /// `watchdog`, the time the armed RTC watchdog leaves the boot.
    pub const fn below(budget: Duration, watchdog: Duration) -> Option<Self> {
        if budget.as_micros() == 0 || budget.as_micros() >= watchdog.as_micros() {
            None
        } else {
            Some(Self(budget))
        }
    }

    /// The budget.
    pub const fn duration(self) -> Duration {
        self.0
    }

    /// Whether `wait` fits the budget at all; a chip checks its waits with
    /// it when it compiles.
    pub const fn admits(self, wait: BootWait) -> bool {
        wait.bound.as_micros() <= self.0.as_micros()
    }
}

/// Why a boot wait ended without its value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitFailure {
    /// The supplied wait bound is zero.
    ZeroBound(&'static str),
    /// The wait passed its own bound.
    TimedOut(&'static str),
    /// The boot deadline passed during the wait, or would pass during a
    /// fixed hold.
    DeadlineExceeded(&'static str),
}

impl WaitFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::ZeroBound(_) => "boot-wait-zero-bound",
            Self::TimedOut(_) => "boot-wait-timeout",
            Self::DeadlineExceeded(_) => "boot-deadline",
        }
    }

    /// The name of the wait that failed.
    pub const fn wait(self) -> &'static str {
        match self {
            Self::ZeroBound(name) | Self::TimedOut(name) | Self::DeadlineExceeded(name) => name,
        }
    }
}

/// The one deadline of the boot, measured on `C`.
pub struct BootDeadline<'clock, C: Clock + ?Sized> {
    clock: &'clock C,
    end: Instant,
}

impl<C: Clock + ?Sized> core::fmt::Debug for BootDeadline<'_, C> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BootDeadline")
            .field("end", &self.end)
            .finish_non_exhaustive()
    }
}

impl<'clock, C: Clock + ?Sized> BootDeadline<'clock, C> {
    /// Start the boot's deadline: `budget` from now on `clock`.
    pub fn start(clock: &'clock C, budget: BootBudget) -> Self {
        Self {
            clock,
            end: clock.now().saturating_add(budget.duration()),
        }
    }

    /// The time left before the deadline.
    pub fn remaining(&self) -> Duration {
        self.end.saturating_duration_since(self.clock.now())
    }

    /// Repeat `attempt` until it yields a value, failing once `wait`'s bound
    /// or the deadline has passed; the first attempt always runs, and so does
    /// one after either limit was seen.
    pub fn poll<T>(
        &self,
        wait: BootWait,
        mut attempt: impl FnMut() -> Option<T>,
    ) -> Result<T, WaitFailure> {
        let limit = self.clock.now().saturating_add(wait.bound);
        loop {
            let now = self.clock.now();
            if let Some(value) = attempt() {
                return Ok(value);
            }
            if now >= self.end {
                return Err(WaitFailure::DeadlineExceeded(wait.name));
            }
            if now >= limit {
                return Err(WaitFailure::TimedOut(wait.name));
            }
        }
    }

    /// Hold for exactly `wait`'s bound: a fixed settle the source states
    /// (a PLL after its calibration, a regulator after its enable). Fails at
    /// once, without waiting, when the hold would end past the deadline.
    pub fn hold(&self, wait: BootWait) -> Result<(), WaitFailure> {
        let start = self.clock.now();
        let end = start
            .checked_add(wait.bound)
            .ok_or(WaitFailure::DeadlineExceeded(wait.name))?;
        if end > self.end {
            return Err(WaitFailure::DeadlineExceeded(wait.name));
        }
        // The hold is the busy-wait itself: the bootstrap has no executor
        // to yield to.
        while self.clock.now() < end {}
        Ok(())
    }
}
