//! IEEE 802.15.4 lifecycle as a client of the shared radio domain.
//!
//! The chain starts from the [`Ieee802154RadioPartition`] of a concurrent
//! split and holds only the MAC, its interrupt route and its ETM channels.
//! Common radio power, the module clocks and the private MAC resets live in
//! the shared radio registers, so each of those transitions borrows the
//! [`RadioDomain`] for its duration and never retains it. The
//! MAC foundation and policy touch only the partition.
//!
//! The public ESP-IDF enable order is `ieee802154_enable` (module clocks),
//! `esp_phy_enable`, `esp_btbb_enable` and then `ieee802154_mac_init`, whose
//! first step is the MAC reset. The PHY client and BTBB steps belong to the
//! PHY layer, which composes them at [`Ieee802154Clocked`]. Completion of this
//! module's states is intentionally weaker than common-PHY, BTBB, RF, or
//! operational-MAC readiness.

#![deny(unsafe_code)]

use super::mac::Ieee802154Pti;
use crate::{
    domain::{
        BtbbAcquired, BtbbError, CoexAbsent, CommonPower, CommonPowerExitFailure,
        CommonRadioPowerError, Ieee802154Client, ModemClockError, PlatformClockProvider,
        RadioDomain,
    },
    ieee802154::{
        backend::Ieee802154MacHal,
        mac::{self, Ieee802154InterruptSetupOwner, Ieee802154TaskOwner},
    },
    phy::RadioPhyRegisters,
    power::PlatformClockBackend,
    root::Ieee802154RadioPartition,
};
use core::fmt;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154FoundationSnapshot,
    Ieee802154LifecycleBackend, Ieee802154MacControl, Ieee802154MacPolicyBackend,
    Ieee802154MacPolicyReadback, Ieee802154MacPolicyWrites, Ieee802154PanIdentity,
    Ieee802154ResetPort, Ieee802154ResetReadback,
};

/// The IEEE 802.15.4 partition split into its task and inactive interrupt
/// halves.
struct Ieee802154Mac {
    task: Ieee802154TaskOwner,
    interrupts: Ieee802154InterruptSetupOwner,
}

impl Ieee802154Mac {
    fn mac_hal(&mut self) -> Ieee802154MacHal<'_> {
        Ieee802154MacHal::from_owned(&mut self.task, &self.interrupts)
    }
}

impl Ieee802154LifecycleBackend for Ieee802154Clocked {
    type ResetPort<'a> = LeasedResetPort<'a>;
    fn mask_all_events(&mut self) {
        self.mac.mac_hal().mask_all_events();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.mac.mac_hal().mask_all_rx_aborts();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.mac.mac_hal().mask_all_tx_aborts();
    }

    fn select_average_ed_sampling(&mut self) {
        self.mac.mac_hal().select_average_ed_sampling();
    }

    fn disable_txrx_coex(&mut self) {
        self.mac
            .mac_hal()
            .set_txrx_pti(Ieee802154Pti::COEX_DISABLED);
    }

    fn disable_ack_coex(&mut self) {
        self.mac.mac_hal().set_ack_pti(Ieee802154Pti::COEX_DISABLED);
    }

    fn apply_txon_delay(&mut self) {
        self.mac.mac_hal().apply_txon_delay();
    }

    fn order_device_accesses(&mut self) {
        self.mac.mac_hal().order_device_accesses();
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.mac.mac_hal().foundation_snapshot()
    }
}

/// The private MAC resets in the shared modem syscon block, borrowed from
/// one lease for one reset transition.
pub struct LeasedResetPort<'a> {
    radio_phy: &'a mut RadioPhyRegisters,
}

impl Ieee802154ResetPort for LeasedResetPort<'_> {
    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.radio_phy.set_ieee802154_mac_reset(asserted);
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.radio_phy.set_ieee802154_apb_reset(asserted);
    }

    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback {
        let reset = self.radio_phy.ieee802154_reset_observation();
        Ieee802154ResetReadback {
            mac_reset_released: reset.mac_reset_released,
            apb_reset_released: reset.apb_reset_released,
        }
    }
}

/// Unpowered IEEE 802.15.4 client holding its partition.
///
/// Construction splits the partition without touching MMIO. It proves
/// neither power nor clocks, common-PHY, BTBB, RF, IRQ, DMA, or MAC
/// readiness.
#[must_use = "the IEEE 802.15.4 client retains its radio partition"]
pub struct Ieee802154Cold {
    mac: Ieee802154Mac,
}

impl Ieee802154Cold {
    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: Ieee802154RadioPartition) -> Self {
        let (task, interrupts) = mac::split(partition.into_mac());
        Self {
            mac: Ieee802154Mac { task, interrupts },
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> Ieee802154RadioPartition {
        let Ieee802154Mac { task, interrupts } = self.mac;
        Ieee802154RadioPartition::from_mac(mac::reunite(task, interrupts))
    }

    /// Enter common radio power as the IEEE 802.15.4 client.
    ///
    /// Only the first client runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power or the first client's sequence failed a
    /// read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        clocks: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<
        (Ieee802154Powered, CommonPower<Ieee802154Client>),
        Ieee802154PowerTransitionFailure<Self>,
    > {
        match radio.enter_common_power(&self.mac.task, clocks) {
            Ok(power) => Ok((Ieee802154Powered { mac: self.mac }, power)),
            Err(error) => Err(Ieee802154PowerTransitionFailure { owner: self, error }),
        }
    }
}

/// IEEE 802.15.4 client inside common radio power.
///
/// It proves only membership in the common modem/PHY power; the module
/// clocks and the MAC resets are later stages.
#[must_use = "the powered IEEE 802.15.4 client must leave common power"]
pub struct Ieee802154Powered {
    mac: Ieee802154Mac,
}

impl Ieee802154Powered {
    /// Leave common radio power, outside coexistence (`coex`); the last
    /// client restores the cold baseline.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client stays entered and the
    /// unchanged owner and proof are returned for a retry.
    #[allow(
        clippy::type_complexity,
        reason = "the failure returns the owner beside its power proof"
    )]
    pub fn power_down<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        power: CommonPower<Ieee802154Client>,
        coex: &CoexAbsent<Ieee802154Client>,
    ) -> Result<
        Ieee802154Cold,
        Ieee802154PowerTransitionFailure<(Self, CommonPower<Ieee802154Client>)>,
    > {
        match radio.exit_common_power(&self.mac.task, power, coex) {
            Ok(()) => Ok(Ieee802154Cold { mac: self.mac }),
            Err(CommonPowerExitFailure { power, error }) => Err(Ieee802154PowerTransitionFailure {
                owner: (self, power),
                error,
            }),
        }
    }
}

impl Ieee802154Powered {
    /// Enable the IEEE 802.15.4 module clocks through the shared planner.
    ///
    /// This is `modem_clock_module_enable(PERIPH_IEEE802154_MODULE)`: only
    /// dependencies no other client holds are switched on; the 160 MHz source
    /// and the analog-I2C master clock go through `platform`.
    ///
    /// # Errors
    ///
    /// The planner rejected the request before any access, or a platform
    /// request failed and poisoned the modem clocks; the owner is returned.
    pub fn enable_clocks<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<Ieee802154Clocked, Ieee802154ClockTransitionFailure<Self>> {
        match radio.enable_modem_clocks(&self.mac.task, platform) {
            Ok(()) => Ok(Ieee802154Clocked { mac: self.mac }),
            Err(error) => Err(Ieee802154ClockTransitionFailure { owner: self, error }),
        }
    }
}

/// Failed module-clock transition retaining the unchanged owner.
#[must_use = "a failed IEEE 802.15.4 clock transition still owns the partition"]
pub struct Ieee802154ClockTransitionFailure<Owner> {
    owner: Owner,
    error: ModemClockError,
}

impl<Owner> Ieee802154ClockTransitionFailure<Owner> {
    pub const fn error(&self) -> ModemClockError {
        self.error
    }

    /// Recover the owner. After [`ModemClockError::Poisoned`] no further
    /// modem clock change succeeds until reset.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for Ieee802154ClockTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154ClockTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed common-power transition retaining the unchanged owner.
#[must_use = "a failed IEEE 802.15.4 power transition still owns the partition"]
pub struct Ieee802154PowerTransitionFailure<Owner> {
    owner: Owner,
    error: CommonRadioPowerError,
}

impl<Owner> Ieee802154PowerTransitionFailure<Owner> {
    pub const fn error(&self) -> CommonRadioPowerError {
        self.error
    }

    /// Recover the owner for a retry.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for Ieee802154PowerTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PowerTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// IEEE 802.15.4 module-clock owner and the MAC's unbound register atoms.
#[must_use = "retain the module-clock and partition owner"]
pub struct Ieee802154Clocked {
    mac: Ieee802154Mac,
}
impl Ieee802154MacPolicyWrites for Ieee802154Clocked {
    fn set_channel(&mut self, channel: oer_espressif_ieee802154_mac_values::Ieee802154Channel) {
        self.mac.mac_hal().set_channel(channel);
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.mac.mac_hal().set_cca_mode(mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.mac.mac_hal().set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.mac.mac_hal().set_mac_control(control);
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.mac.mac_hal().set_ack_timeout(timeout);
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.mac.mac_hal().set_primary_pan_identity(identity);
    }

    fn order_device_accesses(&mut self) {
        self.mac.mac_hal().order_device_accesses();
    }
}

impl Ieee802154MacPolicyBackend for Ieee802154Clocked {
    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback {
        let mut hal = self.mac.mac_hal();
        let foundation = hal.foundation_snapshot();
        let policy = hal.mac_policy_snapshot();
        Ieee802154MacPolicyReadback::new(foundation, policy)
    }
}

impl Ieee802154Clocked {
    /// Release the IEEE 802.15.4 module clocks; clocks another client holds
    /// stay enabled.
    ///
    /// # Errors
    ///
    /// The planner rejected the release before any access, or a platform
    /// request failed and poisoned the modem clocks; the owner is returned.
    pub fn disable_clocks<B: PlatformClockBackend, T>(
        self,
        radio: &mut RadioDomain<B, T>,
    ) -> Result<Ieee802154Powered, Ieee802154ClockTransitionFailure<Self>> {
        match radio.disable_modem_clocks(&self.mac.task) {
            Ok(()) => Ok(Ieee802154Powered { mac: self.mac }),
            Err(error) => Err(Ieee802154ClockTransitionFailure { owner: self, error }),
        }
    }

    /// Lend the shared reset atoms for one family-driver transition.
    pub fn reset_port<'a, B: PlatformClockBackend, T>(
        &self,
        radio: &'a mut RadioDomain<B, T>,
    ) -> LeasedResetPort<'a> {
        LeasedResetPort {
            radio_phy: radio.registers_mut().radio_phy_mut(),
        }
    }

    /// Transfer the unbound MAC atoms, retaining no module-clock release right.
    /// The family driver retains the reset/foundation validation phase.
    pub fn into_operational(self) -> Ieee802154Operational {
        let Ieee802154Mac { task, interrupts } = self.mac;
        Ieee802154Operational { task, interrupts }
    }

    /// Recover the same clock/partition owner after a masked operational epoch.
    /// This performs no MMIO and proves no foundation readback.
    pub fn from_operational(operational: Ieee802154Operational) -> Self {
        let Ieee802154Operational { task, interrupts } = operational;
        Self {
            mac: Ieee802154Mac { task, interrupts },
        }
    }

    /// Join the shared BTBB baseband as the IEEE 802.15.4 client.
    ///
    /// This is `esp_btbb_enable`: only the first holder initializes the
    /// baseband, with `gain`'s byte. The module clocks and resets BTBB
    /// requires are held by this clocked owner.
    ///
    /// # Errors
    ///
    /// As [`crate::domain::RadioDomain::btbb_acquire`].
    pub fn acquire_btbb<B: PlatformClockBackend, T>(
        &self,
        radio: &mut RadioDomain<B, T>,
        gain: &crate::domain::BtbbGain,
    ) -> Result<BtbbAcquired, BtbbError> {
        radio.btbb_acquire(&self.mac.task, gain)
    }

    /// Leave the shared BTBB baseband. This performs no register access.
    ///
    /// # Errors
    ///
    /// IEEE 802.15.4 does not hold BTBB.
    pub fn release_btbb<B: PlatformClockBackend, T>(
        &self,
        radio: &mut RadioDomain<B, T>,
    ) -> Result<(), BtbbError> {
        radio.btbb_release(&self.mac.task)
    }
}

/// The owners of one operational IEEE 802.15.4 MAC epoch.
///
/// The task owner executes commands; the interrupt owner is inactive until
/// it is armed. Neither is RF readiness.
#[must_use = "the operational owners must return through the foundation owner"]
pub struct Ieee802154Operational {
    /// Task-side MAC registers.
    pub task: Ieee802154TaskOwner,
    /// Inactive interrupt ownership for the platform CPU route.
    pub interrupts: Ieee802154InterruptSetupOwner,
}

#[cfg(any(test, feature = "test-support"))]
impl Ieee802154Clocked {
    /// Enter the clocked phase without the modem clock planner, for host
    /// tests of the later stages. It proves no clock.
    #[doc(hidden)]
    pub fn for_host_test(partition: Ieee802154RadioPartition) -> Self {
        let Ieee802154Cold { mac } = Ieee802154Cold::from_partition(partition);
        Self { mac }
    }
}

#[cfg(test)]
mod tests;
