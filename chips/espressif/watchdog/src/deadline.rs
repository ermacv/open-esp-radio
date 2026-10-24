//! Finite system-deadline protection on a main watchdog, independent of any
//! peripheral client.
//!
//! The chip-neutral [`oer_time::deadline`] controller decides when the timer
//! is armed and disarmed; [`DeadlineTimer`] is its backend on one main
//! watchdog. No periodic feed, implicit renewal or Drop-based disarm exists.
//! A caller completes a lease only after its own physical postconditions
//! hold. This is engineering protection while the chip is awake, not a
//! qualified interrupt-to-reset bound. Sleep and external clock changes
//! while armed are unsupported.
//!
//! ```compile_fail,E0382
//! use oer_espressif_watchdog::{MainWatchdogRegisters, deadline::{DeadlineBudget, DeadlineWatchdog}};
//! fn arm_twice<R: MainWatchdogRegisters, C: oer_time::Clock>(
//!     watchdog: DeadlineWatchdog<R, C>,
//!     budget: DeadlineBudget,
//! ) {
//!     let first = watchdog.arm(budget);
//!     let second = watchdog.arm(budget);
//! }
//! ```

pub use oer_time::deadline::{DeadlineBudget, DeadlineError};
use oer_time::{
    Clock,
    deadline::{Backend, Lease, Watchdog},
};

use crate::{MainWatchdog, MainWatchdogRegisters, Microseconds};

/// One main watchdog and the image clock the deadlines are measured on.
pub struct DeadlineTimer<R: MainWatchdogRegisters, C> {
    watchdog: MainWatchdog<R>,
    clock: C,
}

impl<R: MainWatchdogRegisters, C: Clock> Backend for DeadlineTimer<R, C> {
    fn now_micros(&self) -> u64 {
        self.clock.now().as_micros()
    }

    fn arm(&mut self, micros: u32) {
        // The controller arms only with a `DeadlineBudget`'s microseconds,
        // which are nonzero.
        self.watchdog.arm_system_reset(Microseconds(micros));
    }

    fn disarm(&mut self) {
        self.watchdog.disable();
    }
}

/// The exclusive main watchdog owner, moved into its task and then into
/// each lease.
pub struct DeadlineWatchdog<R: MainWatchdogRegisters, C>(Watchdog<DeadlineTimer<R, C>>);

impl<R: MainWatchdogRegisters, C> core::fmt::Debug for DeadlineWatchdog<R, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("DeadlineWatchdog").field(&self.0).finish()
    }
}

impl<R: MainWatchdogRegisters, C: Clock> DeadlineWatchdog<R, C> {
    /// Serve deadlines on `watchdog`, measured on `clock`. The watchdog is
    /// stopped while no lease exists.
    pub const fn new(watchdog: MainWatchdog<R>, clock: C) -> Self {
        Self(Watchdog::new(DeadlineTimer { watchdog, clock }))
    }

    /// Reset the system `budget` from now unless the lease completes first.
    pub fn arm(self, budget: DeadlineBudget) -> Result<DeadlineLease<R, C>, (Self, DeadlineError)> {
        self.0
            .arm(budget)
            .map(DeadlineLease)
            .map_err(|(owner, error)| (Self(owner), error))
    }
}

/// Completion authority retaining the complete watchdog by value.
/// Dropping or forgetting it leaves reset armed.
#[must_use = "only proven restoration may complete the lease; dropping it leaves reset armed"]
pub struct DeadlineLease<R: MainWatchdogRegisters, C>(Lease<DeadlineTimer<R, C>>);

impl<R: MainWatchdogRegisters, C> core::fmt::Debug for DeadlineLease<R, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("DeadlineLease").field(&self.0).finish()
    }
}

impl<R: MainWatchdogRegisters, C: Clock> DeadlineLease<R, C> {
    /// Stop the watchdog and return it, only before the deadline.
    pub fn complete(self) -> Result<DeadlineWatchdog<R, C>, (Self, DeadlineError)> {
        self.0
            .complete()
            .map(DeadlineWatchdog)
            .map_err(|(lease, error)| (Self(lease), error))
    }
}
