//! Finite system-deadline protection, independent of any peripheral client.
//!
//! No periodic feed, implicit renewal, or Drop-based disarm exists. A caller
//! completes a lease only after its own physical postconditions hold. This is
//! engineering protection in the awake XTAL clock domain, not a qualified
//! interrupt-to-reset or reset-to-RF-off bound. Sleep and external clock changes
//! while armed are unsupported. No other TIMG1/Wdt constructor may be used.

pub use oer_time::deadline::{DeadlineBudget, DeadlineError};

#[cfg(feature = "esp32s31")]
mod target {
    use core::mem::ManuallyDrop;
    use esp_hal::{
        peripherals::TIMG1,
        time::{Duration, Instant},
        timer::timg::{MwdtStage, TimerGroup},
    };
    use oer_time::deadline::{Backend, DeadlineBudget, DeadlineError, Lease, Watchdog};

    struct Hardware(ManuallyDrop<TimerGroup<'static, TIMG1<'static>>>);
    impl Backend for Hardware {
        fn now_micros(&self) -> u64 {
            Instant::now().duration_since_epoch().as_micros()
        }
        fn arm(&mut self, micros: u32) {
            self.0
                .wdt
                .set_timeout(MwdtStage::Stage0, Duration::from_micros(u64::from(micros)));
            self.0.wdt.feed();
            self.0.wdt.enable();
        }
        fn disarm(&mut self) {
            self.0.wdt.disable();
        }
    }

    /// Exclusive TIMG1 owner, moved into its task and then into each lease.
    #[derive(Debug)]
    pub struct DeadlineWatchdog(Watchdog<Hardware>);

    impl DeadlineWatchdog {
        pub fn new(timg: TIMG1<'static>) -> Self {
            let mut group = TimerGroup::new(timg);
            group.wdt.disable();
            Self(Watchdog::new(Hardware(ManuallyDrop::new(group))))
        }

        pub fn arm(self, budget: DeadlineBudget) -> Result<DeadlineLease, (Self, DeadlineError)> {
            self.0
                .arm(budget)
                .map(DeadlineLease)
                .map_err(|(owner, error)| (Self(owner), error))
        }
    }

    /// Completion authority retaining the complete watchdog by value.
    /// Dropping or forgetting it leaves reset armed.
    #[derive(Debug)]
    #[must_use = "only proven restoration may complete the lease; dropping it leaves reset armed"]
    pub struct DeadlineLease(Lease<Hardware>);

    impl DeadlineLease {
        pub fn complete(self) -> Result<DeadlineWatchdog, (Self, DeadlineError)> {
            self.0
                .complete()
                .map(DeadlineWatchdog)
                .map_err(|(lease, error)| (Self(lease), error))
        }
    }
}

#[cfg(feature = "esp32s31")]
pub use target::{DeadlineLease, DeadlineWatchdog};
