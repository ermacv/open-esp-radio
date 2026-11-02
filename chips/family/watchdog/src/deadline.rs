//! Finite system-deadline protection owned by the platform watchdog.
//!
//! The pure [`oer_time::deadline`] policy checks time observations; this
//! owner alone programs and stops the hardware reset. No periodic feed,
//! implicit renewal or Drop-based disarm exists. A caller completes a lease
//! only after its own physical postconditions hold. This is engineering
//! protection while awake, not a qualified interrupt-to-reset bound.
//! Sleep and external clock changes while armed are unsupported.
//!
//! ```compile_fail,E0382
//! use oer_espressif_watchdog::{MainWatchdogRegisters, deadline::{DeadlineBudget, DeadlineWatchdog}};
//! fn arm_twice<R: MainWatchdogRegisters, C: oer_time::Clock>(
//!     watchdog: DeadlineWatchdog<R, C>, budget: DeadlineBudget,
//! ) {
//!     let first = watchdog.arm(budget);
//!     let second = watchdog.arm(budget);
//! }
//! ```

pub use oer_time::deadline::{DeadlineBudget, DeadlineError};
use oer_time::{Clock, deadline::Controller};

use crate::{MainWatchdog, MainWatchdogRegisters, Microseconds};

/// The exclusive platform reset owner, moved into its task and each lease.
pub struct DeadlineWatchdog<R: MainWatchdogRegisters, C> {
    watchdog: MainWatchdog<R>,
    clock: C,
    policy: Controller,
}

impl<R: MainWatchdogRegisters, C> core::fmt::Debug for DeadlineWatchdog<R, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeadlineWatchdog").finish_non_exhaustive()
    }
}

impl<R: MainWatchdogRegisters, C: Clock> DeadlineWatchdog<R, C> {
    /// Serve deadlines on `watchdog`, measured on `clock`. The watchdog is
    /// stopped while no lease exists.
    pub const fn new(watchdog: MainWatchdog<R>, clock: C) -> Self {
        Self {
            watchdog,
            clock,
            policy: Controller::new(),
        }
    }

    /// Reset the system `budget` from now unless the lease completes first.
    pub fn arm(
        mut self,
        budget: DeadlineBudget,
    ) -> Result<DeadlineLease<R, C>, (Self, DeadlineError)> {
        let generation = match self.policy.start(self.clock.now(), budget) {
            Ok(generation) => generation,
            Err(error) => return Err((self, error)),
        };
        self.watchdog
            .arm_system_reset(Microseconds(budget.as_micros()));
        Ok(DeadlineLease {
            owner: self,
            generation,
        })
    }
}

/// Completion authority retaining the complete platform reset owner.
/// Dropping or forgetting it leaves reset armed.
///
/// ```compile_fail,E0382
/// use oer_espressif_watchdog::{MainWatchdogRegisters, deadline::DeadlineLease};
/// fn complete_twice<R: MainWatchdogRegisters, C: oer_time::Clock>(lease: DeadlineLease<R, C>) {
///     let first = lease.complete();
///     let second = lease.complete();
/// }
/// ```
#[must_use = "only proven restoration may complete the lease; dropping it leaves reset armed"]
pub struct DeadlineLease<R: MainWatchdogRegisters, C> {
    owner: DeadlineWatchdog<R, C>,
    generation: u64,
}

impl<R: MainWatchdogRegisters, C> core::fmt::Debug for DeadlineLease<R, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeadlineLease")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl<R: MainWatchdogRegisters, C: Clock> DeadlineLease<R, C> {
    /// Stop the watchdog and return it, only before the deadline. Failure
    /// returns the lease with its reset still armed.
    pub fn complete(mut self) -> Result<DeadlineWatchdog<R, C>, (Self, DeadlineError)> {
        match self
            .owner
            .policy
            .complete(self.owner.clock.now(), self.generation)
        {
            Ok(()) => {
                self.owner.watchdog.disable();
                Ok(self.owner)
            }
            Err(error) => Err((self, error)),
        }
    }
}
