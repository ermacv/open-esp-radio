//! Finite system-deadline protection, independent of any peripheral client.
//!
//! No periodic feed, implicit renewal, or Drop-based disarm exists. A caller
//! completes a lease only after its own physical postconditions hold. This is
//! engineering protection in the awake XTAL clock domain, not a qualified
//! interrupt-to-reset or reset-to-RF-off bound. Sleep and external clock changes
//! while armed are unsupported. No other TIMG1/Wdt constructor may be used.

pub use oer_soc_deadline::{DeadlineBudget, DeadlineError};

#[cfg(feature = "esp32s31")]
mod target {
    use core::{cell::RefCell, mem::ManuallyDrop};
    use critical_section::Mutex;
    use esp_hal::{
        peripherals::TIMG1,
        time::{Duration, Instant},
        timer::timg::{MwdtStage, TimerGroup},
    };
    use oer_soc_deadline::{Backend, Controller, DeadlineBudget, DeadlineError};

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

    /// Exclusive TIMG1 owner. Share this service, never the peripheral itself.
    /// A critical section serializes finite programming only; expiry needs no
    /// ISR, executor, Host progress or access to this service.
    pub struct DeadlineWatchdog(Mutex<RefCell<Controller<Hardware>>>);

    impl DeadlineWatchdog {
        pub fn new(timg: TIMG1<'static>) -> Self {
            let mut group = TimerGroup::new(timg);
            group.wdt.disable();
            Self(Mutex::new(RefCell::new(Controller::new(Hardware(
                ManuallyDrop::new(group),
            )))))
        }

        /// Arm once. Reject an existing obligation without changing its timer.
        /// The pinned HAL selects XTAL for S31. The logical completion deadline
        /// is sampled before programming; physical expiry additionally includes
        /// programming/latch/reset latency, which requires hardware measurement.
        pub fn arm(&self, budget: DeadlineBudget) -> Result<DeadlineLease<'_>, DeadlineError> {
            let generation =
                critical_section::with(|cs| self.0.borrow(cs).borrow_mut().arm(budget))?;
            Ok(DeadlineLease {
                owner: self,
                generation,
            })
        }
    }

    /// Unique completion authority for one operation. Dropping or forgetting it
    /// leaves the watchdog armed and prevents any later operation from renewing
    /// the deadline. This token is bound to the service that issued it.
    #[must_use = "only proven restoration may complete the lease; dropping it leaves reset armed"]
    pub struct DeadlineLease<'a> {
        owner: &'a DeadlineWatchdog,
        generation: u64,
    }

    impl DeadlineLease<'_> {
        /// Disarm after the client's physical postconditions hold. Equality or
        /// lateness rejects completion and leaves the hardware armed.
        pub fn complete(self) -> Result<(), DeadlineError> {
            critical_section::with(|cs| {
                self.owner
                    .0
                    .borrow(cs)
                    .borrow_mut()
                    .complete(self.generation)
            })
        }
    }
}

#[cfg(feature = "esp32s31")]
pub use target::{DeadlineLease, DeadlineWatchdog};
