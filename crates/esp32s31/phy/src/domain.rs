//! One registered PHY epoch shared by every protocol client.
//!
//! A [`PhyDomain`] is the target-issued [`RegisteredPhyState`] and the client
//! set that shares it. The concurrent domain keeps it under the radio arbiter;
//! Wi-Fi, Bluetooth and IEEE 802.15.4 acquire and release their client bits
//! in it.

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{PhyClientSnapshot, PhyClientState, PhyTrackTimeError},
};

/// One registered PHY epoch: the target-issued calibration state and the
/// client set that shares it.
///
/// The radio arbiter holds the domain beside the shared PHY registers, and
/// the registration path mints it. The domain is neither `Clone` nor
/// constructible outside the crate, so its calibration state and client set
/// cannot be replaced or paired with another epoch.
///
/// ```compile_fail
/// use oer_esp32s31_phy::domain::PhyDomain;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<PhyDomain>();
/// ```
#[must_use = "a registered PHY domain is the unique owner of its epoch"]
pub struct PhyDomain {
    pub(crate) registered: RegisteredPhyState,
    pub(crate) clients: PhyClientState,
}

impl PhyDomain {
    pub(crate) const fn new(registered: RegisteredPhyState, clients: PhyClientState) -> Self {
        Self {
            registered,
            clients,
        }
    }

    /// Mint the domain of one completed target registration.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        let epoch = witness.epoch();
        Self::new(
            RegisteredPhyState::from_target_completion(state, witness),
            PhyClientState::for_registration(
                crate::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
                epoch,
            ),
        )
    }

    /// Borrow the registered calibration state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Set the vendor's temperature-tracking debug policy bytes.
    #[cfg(all(feature = "tracking-diagnostics", target_arch = "riscv32"))]
    pub(crate) fn set_temperature_tracking_debug(&mut self, first: u8, second: u8) {
        self.registered
            .target_state_mut()
            .set_temperature_tracking_debug(first, second);
    }

    /// Inspect the shared client set without exposing its raw mask.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Replace an older cache for this epoch with the currently committed
    /// semantic calibration state.
    ///
    /// Consuming the prior cache preserves the single-owner persistence
    /// contract. Its platform-derived identity is retained; the next cold
    /// registration still validates that identity against the physical chip.
    pub fn refresh_calibration_cache(
        &self,
        previous: crate::state::PhyCalibrationCache,
    ) -> crate::state::PhyCalibrationCache {
        self.registered
            .state()
            .calibration_cache(previous.identity())
    }

    /// Inspect registered-policy conditions without sampling temperature,
    /// advancing deadlines or acquiring RF. Values describe retained state,
    /// not a job plan.
    pub fn inspect_tracking(
        &self,
        now_micros: u64,
    ) -> Result<crate::tracking::inspection::Inspection, PhyTrackTimeError> {
        crate::tracking::inspection::Inspection::registered(
            &self.registered,
            self.client_snapshot(),
            now_micros,
        )
    }

    /// Wait for tracking demand without transferring the domain to a timer.
    ///
    /// Cancellation and timer errors leave the domain unchanged. The returned
    /// observation is not a hardware grant: obtain the required physical
    /// exclusion before invoking a consuming evaluation. No timestamp is
    /// refreshed here.
    pub async fn wait_for_tracking_demand(
        &self,
        timer: &impl oer_time::Timer,
    ) -> Result<Option<crate::tracking::schedule::Demand>, PhyTrackTimeError> {
        use crate::tracking::schedule::Schedule;
        loop {
            match self
                .client_snapshot()
                .tracking_schedule_at(timer.now().as_micros())?
            {
                Schedule::Inactive => return Ok(None),
                Schedule::Due(demand) => return Ok(Some(demand)),
                Schedule::At(deadline) => {
                    timer
                        .wait_until(oer_time::Instant::from_micros(deadline))
                        .await;
                }
            }
        }
    }
}
