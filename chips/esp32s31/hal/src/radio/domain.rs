//! The shared radio domain of concurrently running protocols.
//!
//! [`RadioDomain`] owns the shared radio registers together with the shared
//! PHY software state (registration epoch and calibration restore slots).
//! Protocol drivers hold only their own register partitions; the one owner of
//! the domain, the radio system, transfers an owned PHY transaction for
//! lifecycle and tracking. Client operations take short exclusive borrows;
//! no lock is shared with an interrupt handler.
//!
//! The domain also owns common radio power. Each protocol enters and leaves
//! it proving its identity with its own register set; the first client runs
//! the modem/PHY power sequence once and the last restores the cold-power
//! baseline. Refcounted modem clock dependencies are the shared modem clock
//! planner's, and are not part of this membership.
//!
//! Modem clocks are reference-counted per dependency by the shared modem clock
//! planner inside the domain: each client's module clocks are enabled and
//! disabled through it, and a client leaving never disables a clock another
//! client still holds. The platform clock provider supplies the upstream
//! 160 MHz source and the analog-I2C master clock.
//!
//! The domain is not a coexistence grant: it serializes register access, not
//! air time.

use crate::radio::ieee80211::registers::WifiRadioRegisters;

use crate::power::clock::{
    ModemClockModule, ModemClockPlannerIdentity, ModemExecution, ModemPlatformClocks,
};
pub use crate::power::radio::{
    BluetoothClient, CommonPower, CommonPowerExitFailure, CommonRadioPowerError, Ieee802154Client,
    RadioClient, RadioClientKind, WifiClient,
};
pub use crate::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};
#[cfg(test)]
use crate::radio::coex::PhyGrantProtectError;
use crate::radio::coex::participation::CoexParticipants;
pub use crate::radio::coex::participation::{CoexAbsent, Coexisting, ExternalCoexistence};
use crate::radio::coex::{
    CoexClockPort, CoexEventId, CoexPolicyTimerBank, CoexPti, CoexPtiTable,
    PHY_GRANT_PROTECT_EVENT, PhyGrantProtect,
};
use crate::{
    power::radio::CommonRadioPower, radio::phy::restore::PhyRouteState,
    system::owner::PhyRegistrationEpoch,
};

pub mod transaction;
use transaction::PhyIdentity;

mod sealed {
    pub trait RadioClientOwner {}
}

/// Protocol register owner that proves which client calls the domain.
///
/// Only the protocol register sets implement it, so a caller cannot enter or
/// leave common power on behalf of a protocol whose registers it lacks.
pub trait RadioClientOwner: sealed::RadioClientOwner {
    /// The client these registers belong to.
    type Client: RadioClientKind;
    const CLIENT: RadioClient = <Self::Client as RadioClientKind>::CLIENT;
}

impl sealed::RadioClientOwner for WifiRadioRegisters {}
impl RadioClientOwner for WifiRadioRegisters {
    type Client = WifiClient;
}

/// Shared registers, the PHY and power state that must stay with them,
/// and the upper layer's attachment.
struct SharedRadioState<B: PlatformClockBackend, T> {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower<B>,
    clocks: ModemClocks,
    /// Platform references of the modem clock planner's enabled
    /// dependencies; a poisoned planner keeps them held.
    modem_platform_clocks: ModemPlatformClocks<B>,
    coex_pti: CoexPtiTable,
    phy_grant_protected: bool,
    external_coex: Option<crate::radio::coex::ExternalCoexConfig>,
    coex_participants: CoexParticipants,
    attachment: T,
    identity: PhyIdentity,
}

impl<B: PlatformClockBackend, T> SharedRadioState<B, T> {
    /// The platform clock references the radio holds: common power and the
    /// modem clock dependencies (also while poisoned).
    fn platform_clock_holds(&self) -> PlatformClockHolds {
        let mut holds = PlatformClockHolds::default();
        self.power.count_platform_clocks(&mut holds);
        self.modem_platform_clocks.count_into(&mut holds);
        holds
    }
}

type ModemClocks =
    oer_chips_family_radio_power::modem::ModemClocks<crate::power::clock::Dependency, CLOCK_SLOTS>;

/// Why a module's modem clocks cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockError {
    /// The module already holds its modem clocks.
    AlreadyEnabled,
    /// The module does not hold its modem clocks.
    NotEnabled,
    /// The planner rejected the request before any register access.
    Rejected,
    /// A platform clock request failed at a physical edge; modem clocks are
    /// poisoned until reset.
    Poisoned,
}

impl oer_portable_diagnostics::DiagnosticPath for ModemClockError {
    fn push_path(&self, path: &mut oer_portable_diagnostics::DiagPath) {
        path.name(match self {
            Self::AlreadyEnabled => "AlreadyEnabled",
            Self::NotEnabled => "NotEnabled",
            Self::Rejected => "Rejected",
            Self::Poisoned => "Poisoned",
        });
    }
}

/// Modem clock modules the shared PHY domain requests for itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClockModule {
    /// `PERIPH_PHY_MODULE`: ESP-IDF's `wifi_bt_common_module_enable`, held
    /// from the first `esp_phy_enable` until the last `esp_phy_disable`
    /// closes RF.
    Phy,
    /// `PERIPH_PHY_CALIBRATION_MODULE`: ESP-IDF's `phy_module_enable`, held
    /// only while the first client's calibration or RF wake runs.
    Calibration,
}

/// One modem clock lease slot per protocol client, per PHY-domain module and for the
/// coexistence module of external coexistence.
const CLOCK_SLOTS: usize = 6;

/// The coexistence module clock ESP-IDF's `coex_module_enable` holds while
/// external coexistence runs.
const EXTERNAL_COEX_SLOT: usize = 5;

const fn client_index(client: RadioClient) -> usize {
    match client {
        RadioClient::Wifi => 0,
        RadioClient::Bluetooth => 1,
        RadioClient::Ieee802154 => 2,
    }
}

const fn client_module(client: RadioClient) -> ModemClockModule {
    match client {
        RadioClient::Wifi => ModemClockModule::Wifi,
        RadioClient::Bluetooth => ModemClockModule::Bluetooth,
        RadioClient::Ieee802154 => ModemClockModule::Ieee802154,
    }
}

const fn phy_index(module: PhyClockModule) -> usize {
    match module {
        PhyClockModule::Phy => 3,
        PhyClockModule::Calibration => 4,
    }
}

const fn phy_module(module: PhyClockModule) -> ModemClockModule {
    match module {
        PhyClockModule::Phy => ModemClockModule::Phy,
        PhyClockModule::Calibration => ModemClockModule::PhyCalibration,
    }
}

/// The shared radio domain: the shared radio registers, the shared PHY
/// software state and common radio power, owned by value.
///
/// ```compile_fail
/// use oer_chips_esp32s31_hal::{radio::domain::RadioDomain, power::PlatformClockBackend};
/// fn requires_clone<T: Clone>() {}
/// fn cannot_clone<B: PlatformClockBackend>() {
///     requires_clone::<RadioDomain<B>>();
/// }
/// ```
///
/// `B` binds every platform clock reference to one backend. Providers used
/// with this domain must have `Backend = B`.
///
/// `T` is state an upper layer keeps with the domain, such as the PHY
/// layer's registered domain. The HAL never interprets it.
#[must_use = "dropping the radio domain permanently loses the shared registers"]
pub struct RadioDomain<B: PlatformClockBackend, T = ()> {
    state: SharedRadioState<B, T>,
}

impl<B: PlatformClockBackend, T> RadioDomain<B, T> {
    /// Place the shared owner, its PHY state and `attachment` in one domain.
    /// This performs no MMIO.
    pub(crate) fn new(
        registers: SharedRadioRegisters,
        phy: PhyRouteState,
        attachment: T,
        identity: PhyIdentity,
        clock_identity: ModemClockPlannerIdentity,
    ) -> Self {
        Self {
            state: SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
                clocks: ModemClocks::new(clock_identity),
                modem_platform_clocks: ModemPlatformClocks::default(),
                coex_pti: CoexPtiTable::VENDOR,
                phy_grant_protected: false,
                external_coex: None,
                coex_participants: CoexParticipants::default(),
                attachment,
                identity,
            },
        }
    }

    /// Give the parts back, once nothing holds them.
    ///
    /// # Errors
    ///
    /// Returns the unchanged domain while a client still holds common power
    /// or a PHY calibration still owns a restore obligation.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the parts, or the unchanged domain, are returned exactly as held"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> Result<(SharedRadioRegisters, PhyRouteState, T), (Self, RadioDomainReleaseError)> {
        let state = self.state;
        if state.power.any_client() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::CommonPowerHeld,
            ));
        }
        if state.clocks.any_held() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::ModemClocksHeld,
            ));
        }
        if state.phy_grant_protected {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::PhyGrantProtectHeld,
            ));
        }
        if state.external_coex.is_some() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::ExternalCoexActive,
            ));
        }
        if let Err(error) = crate::system::root::check_phy_restore_complete(&state.phy) {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::Restore(error),
            ));
        }
        if !state.platform_clock_holds().is_empty() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::PlatformClocksHeld,
            ));
        }
        Ok((state.registers, state.phy, state.attachment))
    }

    fn from_state(state: SharedRadioState<B, T>) -> Self {
        Self { state }
    }

    #[cfg(test)]
    pub(crate) fn phy_state_mut_for_test(&mut self) -> &mut PhyRouteState {
        &mut self.state.phy
    }

    /// Begin a synthetic registration for a hermetic host ownership model.
    #[cfg(all(any(test, feature = "test-support"), not(target_arch = "riscv32")))]
    pub fn begin_registration_for_host_test(
        &mut self,
    ) -> crate::system::owner::PhyRegistrationGrant {
        crate::system::owner::PhyRegistrationGrant::new(self.state.phy.begin_registration_epoch())
    }

    /// Enable the Wi-Fi receive path on this owned domain.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_wifi_rx(&mut self) {
        crate::radio::ieee80211::baseband::enable_wifi(self.state.registers.radio_phy_mut());
    }

    /// Disable the Wi-Fi receive path on this owned domain.
    #[cfg(target_arch = "riscv32")]
    pub fn disable_wifi_rx(&mut self) {
        crate::radio::ieee80211::baseband::disable_wifi(self.state.registers.radio_phy_mut());
    }

    /// Record a programmed PHY grant-protect request without register
    /// access, so a host test or a probe image can check the release alone.
    #[cfg(test)]
    #[doc(hidden)]
    pub fn hold_phy_grant_protect_for_host_test(&mut self) {
        self.state.phy_grant_protected = true;
    }

    /// Keep every platform clock reference the radio holds on until a chip
    /// reset, and forget them: the explicit end of a terminal validation
    /// image or test that drops its radio domain. Each guard ends with
    /// `abandon`, so none is dropped.
    #[cfg(test)]
    #[doc(hidden)]
    pub fn abandon_platform_clocks(&mut self) {
        let state = &mut self.state;
        state.power.abandon_platform_clocks();
        core::mem::take(&mut state.modem_platform_clocks).abandon();
    }

    /// Take a platform reference into common power as a failed power
    /// sequence leaves it.
    #[cfg(test)]
    pub(crate) fn hold_platform_clock_for_test(
        &mut self,
        clock: PlatformClock,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) {
        self.state
            .power
            .hold_platform_clock_for_test(clock, platform);
    }

    #[cfg(test)]
    pub(crate) fn hold_common_power_for_test(&mut self, client: RadioClient) {
        self.state.power.hold_for_test(client);
    }
}

/// Why the radio domain cannot give its parts back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioDomainReleaseError {
    /// A client still holds common radio power.
    CommonPowerHeld,
    /// A client still holds modem clocks, or they are poisoned.
    ModemClocksHeld,
    /// The PHY grant-protect request is still programmed.
    PhyGrantProtectHeld,
    /// External coexistence still runs.
    ExternalCoexActive,
    /// The radio still holds a platform clock reference no client accounts
    /// for, such as the sources a failed common power sequence took. The
    /// reunion waits, as for the other holds: a retried power-up reuses
    /// those sources, and its client's exit gives them back.
    PlatformClocksHeld,
    /// A PHY calibration still owns a restore obligation.
    Restore(crate::system::root::RadioPhyReleaseError),
}

impl<B: PlatformClockBackend, T> RadioDomain<B, T> {
    fn state(&self) -> &SharedRadioState<B, T> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SharedRadioState<B, T> {
        &mut self.state
    }

    /// The registration that currently describes the shared PHY.
    pub fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        self.state().phy.registration_epoch()
    }

    /// Whether `client` currently holds common radio power.
    pub fn holds_common_power(&self, client: RadioClient) -> bool {
        self.state().power.holds(client)
    }

    /// The platform clock references the radio holds: common power and the
    /// modem clock dependencies (also while poisoned).
    pub fn platform_clock_holds(&self) -> PlatformClockHolds {
        self.state().platform_clock_holds()
    }

    /// Enter common radio power as the protocol that owns `owner`.
    ///
    /// Only the first client runs the modem/PHY power sequence. Common power
    /// does not reset the Wi-Fi MAC; that reset belongs to Wi-Fi's own entry.
    ///
    /// The first client also takes the platform's 160 MHz, MPLL and
    /// analog-I2C references from `clocks`; ESP-HAL alone writes those shared
    /// gates.
    ///
    /// The returned proof is the client's membership; only
    /// [`Self::exit_common_power`] consumes it.
    ///
    /// # Errors
    ///
    /// The client already holds power, or the first client's sequence failed
    /// a read-back checkpoint or a platform reference and admitted no client.
    pub fn enter_common_power<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
        clocks: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<CommonPower<O::Client>, CommonRadioPowerError> {
        let state = self.state_mut();
        state
            .power
            .enter::<O::Client>(state.registers.radio_phy_mut(), clocks)
    }

    /// Wi-Fi's checked cold-reset edge followed by common power. The whole
    /// transition stays under this exclusive domain borrow with no yield.
    pub(crate) fn enter_wifi_power(
        &mut self,
        _owner: &WifiRadioRegisters,
        clocks: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<CommonPower<WifiClient>, CommonRadioPowerError> {
        let state = self.state_mut();
        state
            .power
            .enter_wifi(state.registers.radio_phy_mut(), clocks)
    }

    /// How many radios take part in coexistence.
    pub fn coex_participant_count(&self) -> u32 {
        self.state().coex_participants.count()
    }

    /// Enter client `K` into the coexistence participant set; returns how many
    /// take part now. The shared radio's owner applies the coexistence policy.
    pub fn enter_coex<K: RadioClientKind>(
        &mut self,
        absent: CoexAbsent<K>,
    ) -> (Coexisting<K>, u32) {
        self.state_mut().coex_participants.enter_client(absent)
    }

    /// Leave client `K` from the coexistence participant set; returns how
    /// many still take part.
    pub fn leave_coex<K: RadioClientKind>(
        &mut self,
        coexisting: Coexisting<K>,
    ) -> (CoexAbsent<K>, u32) {
        self.state_mut().coex_participants.leave_client(coexisting)
    }

    /// Leave common radio power as the protocol that owns `owner`.
    ///
    /// The last client gives back the platform's analog-I2C reference,
    /// restores the cold-power baseline captured before the first power edge
    /// and then gives back the 160 MHz and MPLL references.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client then stays entered and the
    /// proof is returned so the exit can be retried. Releasing a platform
    /// reference gives its guard back explicitly and cannot fail.
    pub fn exit_common_power<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
        power: CommonPower<O::Client>,
        _coex: &CoexAbsent<O::Client>,
    ) -> Result<(), CommonPowerExitFailure<O::Client>> {
        let state = self.state_mut();
        state.power.exit(state.registers.radio_phy_mut(), power)
    }

    /// Borrow the shared PHY registers with the route restore state, for a
    /// test that also borrows a protocol's own registers.
    #[cfg(test)]
    pub(crate) fn phy_parts_mut(
        &mut self,
    ) -> (
        &mut crate::radio::phy::RadioPhyRegisters,
        &mut PhyRouteState,
    ) {
        let state = self.state_mut();
        (state.registers.radio_phy_mut(), &mut state.phy)
    }

    /// Enable the modem clocks of the protocol that owns `owner`.
    ///
    /// This is ESP-IDF's reference-counted `modem_clock_module_enable` for the
    /// client's module: the monotonic modem ICG maps are ORed in, then only
    /// dependencies at zero references are enabled. The upstream 160 MHz
    /// source and the analog-I2C master clock go through `platform`.
    ///
    /// # Errors
    ///
    /// The client already holds its clocks, the planner rejected the request
    /// before any access, or a platform request failed and poisoned the modem
    /// clocks.
    pub fn enable_modem_clocks<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), ModemClockError> {
        self.enable_slot(client_index(O::CLIENT), client_module(O::CLIENT), platform)
    }

    /// Disable the modem clocks of the protocol that owns `owner`; only
    /// dependencies no other module holds are disabled.
    ///
    /// # Errors
    ///
    /// The client does not hold its clocks, the planner rejected the release
    /// before any access, or the modem clocks are poisoned. Platform
    /// references are given back explicitly through their guards; a poisoned
    /// release keeps the references it did not reach.
    pub fn disable_modem_clocks<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
    ) -> Result<(), ModemClockError> {
        self.disable_slot(client_index(O::CLIENT))
    }

    /// Record whether Wi-Fi is initialized, as ESP-IDF's
    /// `modem_clock_configure_wifi_status` does after `esp_wifi_init` and in
    /// `esp_wifi_deinit`. While it is, disabling a module keeps the Wi-Fi
    /// clock dependencies' hardware enabled.
    ///
    /// # Errors
    ///
    /// The modem clocks are poisoned.
    pub fn set_wifi_initialized(
        &mut self,
        _owner: &WifiRadioRegisters,
        initialized: crate::radio::ieee80211::client::WifiInitialization,
    ) -> Result<(), ModemClockError> {
        // The family planner's one translation of the name into its flag.
        self.state_mut()
            .clocks
            .set_wifi_initialized(matches!(
                initialized,
                crate::radio::ieee80211::client::WifiInitialization::Initialized
            ))
            .map_err(modem_error)
    }

    /// Enable a modem clock module of the shared PHY domain, with the same
    /// reference-counted transaction as a client's module.
    ///
    /// # Errors
    ///
    /// As [`Self::enable_modem_clocks`], for the PHY module's own slot.
    pub fn enable_phy_modem_clocks(
        &mut self,
        module: PhyClockModule,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), ModemClockError> {
        self.enable_slot(phy_index(module), phy_module(module), platform)
    }

    /// Start external coexistence with `config`, the register half of
    /// ESP-IDF's `esp_enable_extern_coex_gpio_pin` after the caller routed
    /// the signals: the coexistence module clock (`coex_module_enable`), the
    /// work mode and grant (`esp_coex_external_params`), then the priorities
    /// and the enable of `esp_coex_external_set`.
    ///
    /// The returned [`ExternalCoexistence`] takes part in coexistence until
    /// [`Self::stop_external_coex`], with how many radios take part now.
    ///
    /// # Errors
    ///
    /// External coexistence already runs, or the module clock failed; no
    /// register changed.
    pub fn start_external_coex(
        &mut self,
        config: crate::radio::coex::ExternalCoexConfig,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(ExternalCoexistence, u32), crate::radio::coex::ExternalCoexError> {
        let state = self.state_mut();
        oer_chips_family_radio_power::domain::start_external(
            &mut state.external_coex,
            config,
            &mut ExternalBinding {
                registers: &mut state.registers,
                clocks: &mut state.clocks,
                held: &mut state.modem_platform_clocks,
                platform,
            },
        )?;
        Ok(state.coex_participants.enter_external())
    }

    /// Stop external coexistence, the register half of
    /// `esp_coex_external_stop`: clear the priorities, disable, then release
    /// the coexistence module clock.
    ///
    /// # Errors
    ///
    /// The planner rejected releasing the module clock after the registers
    /// were stopped; external coexistence left the
    /// participant set either way. On success it returns how many radios
    /// still take part.
    pub fn stop_external_coex(
        &mut self,
        external: ExternalCoexistence,
    ) -> Result<u32, crate::radio::coex::ExternalCoexError> {
        let state = self.state_mut();
        let remaining = state.coex_participants.leave_external(external);
        oer_chips_family_radio_power::domain::stop_external(
            &mut state.external_coex,
            &mut ExternalBinding {
                registers: &mut state.registers,
                clocks: &mut state.clocks,
                held: &mut state.modem_platform_clocks,
                platform: (),
            },
        )?;
        Ok(remaining)
    }

    /// The running external coexistence, if any.
    pub fn external_coex(&self) -> Option<crate::radio::coex::ExternalCoexConfig> {
        self.state().external_coex
    }

    /// Disable a modem clock module of the shared PHY domain.
    ///
    /// # Errors
    ///
    /// As [`Self::disable_modem_clocks`], for the PHY module's own slot.
    pub fn disable_phy_modem_clocks(
        &mut self,
        module: PhyClockModule,
    ) -> Result<(), ModemClockError> {
        self.disable_slot(phy_index(module))
    }

    fn enable_slot(
        &mut self,
        index: usize,
        module: ModemClockModule,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), ModemClockError> {
        let state = self.state_mut();
        state
            .clocks
            .enable(
                index,
                crate::power::clock::module_dependencies(module),
                &mut ModemExecution {
                    port: state.registers.radio_phy_mut(),
                    held: &mut state.modem_platform_clocks,
                    platform,
                },
            )
            .map_err(modem_error)
    }

    fn disable_slot(&mut self, index: usize) -> Result<(), ModemClockError> {
        let state = self.state_mut();
        state
            .clocks
            .disable(
                index,
                &mut ModemExecution {
                    port: state.registers.radio_phy_mut(),
                    held: &mut state.modem_platform_clocks,
                    platform: (),
                },
            )
            .map_err(modem_error)
    }

    /// The current coexistence priority of `event`.
    pub fn coex_pti(&self, event: CoexEventId) -> CoexPti {
        self.state().coex_pti.pti(event)
    }

    /// The complete coexistence priority table.
    pub fn coex_pti_table(&self) -> CoexPtiTable {
        self.state().coex_pti
    }

    /// Replace the coexistence priority of `event`, as `coex_pti_set` does.
    ///
    /// Only the table changes; a protocol republishes its MAC PTI registers
    /// from the new value.
    pub fn set_coex_pti(&mut self, event: CoexEventId, pti: CoexPti) {
        self.state_mut().coex_pti.set(event, pti);
    }

    /// Borrow the PHY grant-protect request for one maintenance operation.
    #[cfg(test)]
    pub fn phy_grant_protect(&mut self) -> PhyGrantProtect<'_> {
        let state = self.state_mut();
        let pti = state.coex_pti.pti(PHY_GRANT_PROTECT_EVENT);
        PhyGrantProtect::new(
            state.registers.coex_timers_mut(),
            pti,
            &mut state.phy_grant_protected,
        )
    }

    /// Program the PHY grant-protect request ([`PhyGrantProtect::acquire`]).
    ///
    /// # Errors
    ///
    /// The request is already programmed; nothing is written.
    #[cfg(test)]
    pub fn acquire_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().acquire()
    }

    /// Withdraw the PHY grant-protect request ([`PhyGrantProtect::release`]).
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
    #[cfg(test)]
    pub fn release_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().release()
    }

    /// Whether the PHY grant-protect request is programmed.
    pub fn phy_grant_protected(&self) -> bool {
        self.state().phy_grant_protected
    }

    /// Lend disjoint policy-timer and clock owners for one coexistence transaction.
    /// Timer 5 remains reserved for PHY grant protection and cannot be selected
    /// through the policy port. Both loans prevent any other domain access.
    ///
    /// ```compile_fail,E0499
    /// use oer_chips_esp32s31_hal::{radio::domain::RadioDomain, power::PlatformClockBackend};
    /// fn overlap<B: PlatformClockBackend, T>(radio: &mut RadioDomain<B, T>) {
    ///     let (mut timers, mut clock) = radio.coex_ports();
    ///     let _other = radio.coex_ports();
    ///     let _ = clock.sample_low_power_clock();
    ///     timers.enable(oer_chips_esp32s31_hal::radio::coex::CoexPolicyTimer::Timer0);
    /// }
    /// ```
    pub fn coex_ports(&mut self) -> (CoexPolicyTimerBank<'_>, CoexClockPort<'_>) {
        let state = self.state_mut();
        let (phy, timers) = state.registers.radio_phy_and_coex_timers_mut();
        (
            CoexPolicyTimerBank::new(timers, &state.coex_pti),
            CoexClockPort::new(phy),
        )
    }

    /// Borrow the upper layer's attachment.
    pub fn attachment(&self) -> &T {
        &self.state().attachment
    }

    /// Mutably borrow the upper layer's attachment.
    pub fn attachment_mut(&mut self) -> &mut T {
        &mut self.state_mut().attachment
    }

    /// The radio's power, clock and reset words under this domain, and the
    /// modem register bus: its words and its proof when its gate words read
    /// open
    /// ([`crate::radio::phy::RadioPhyRegisters::observe_power`]). Writes
    /// nothing.
    pub fn observe_power(
        &self,
    ) -> (
        crate::radio::phy::power_words::RadioPowerWords,
        crate::radio::phy::power_words::ModemBus<'_>,
    ) {
        self.registers().radio_phy().observe_power()
    }

    /// Reset the Wi-Fi MAC's block for its member, whose clocks `_clocks`
    /// proves on, for the first time since their clocking (`first`, one a
    /// clocking), and write back what the block's other owners keep in it;
    /// `cbw` is the channel the member is tuned to, lowered for the PHY. The
    /// answer is the reset's proof, which the member's cold preparation
    /// takes. A few writes and one read, with no wait and no failure.
    ///
    /// The pulse is `modem_clock_module_mac_reset(PERIPH_WIFI_MODULE)`'s,
    /// through `modem_syscon_ll_reset_wifimac`; the write-back is
    /// [`SharedRadioRegisters::write_back_wifi_mac_foreign_words`].
    pub fn reset_wifi_mac_block(
        &mut self,
        _clocks: &crate::radio::ieee80211::client::WifiClocksOn,
        first: crate::radio::ieee80211::client::MacFirstReset,
        cbw: u8,
    ) -> crate::radio::ieee80211::init::MacBlockReset {
        let crate::radio::ieee80211::client::MacFirstReset { _private: () } = first;
        reset_wifi_mac_block(self.registers_mut(), cbw)
    }

    /// Reset the Wi-Fi MAC's block after its member's stop: the walker's
    /// edge, then the MAC's stop request and its hold, which `stop` proves.
    /// The same pulse and write-back as [`Self::reset_wifi_mac_block`]; the
    /// answer adds the proof the receive ring's release takes.
    pub fn reset_stopped_wifi_mac_block(
        &mut self,
        _clocks: &crate::radio::ieee80211::client::WifiClocksOn,
        stop: crate::radio::ieee80211::mac::StopRequested,
        cbw: u8,
    ) -> (
        crate::radio::ieee80211::init::MacBlockReset,
        crate::radio::ieee80211::init::MacStopReset,
    ) {
        reset_stopped_wifi_mac_block(self.registers_mut(), stop, cbw)
    }

    /// Read the three words inside the `WIFI_MAC` block that this domain
    /// owns, without effect, for a probe's observation of the block's reset.
    pub fn foreign_wifi_mac_words(&self) -> crate::radio::modem::ForeignWifiMacWords {
        self.registers().foreign_wifi_mac_words()
    }

    pub(crate) fn registers(&self) -> &SharedRadioRegisters {
        &self.state().registers
    }

    /// Borrow the shared registers for a protocol transaction that also
    /// touches protocol registers.
    pub(crate) fn registers_mut(&mut self) -> &mut SharedRadioRegisters {
        &mut self.state_mut().registers
    }
}

/// The writes of [`RadioDomain::reset_wifi_mac_block`].
pub(crate) trait MacBlockResetPort {
    /// `RST_WIFIMAC`, asserted or released.
    fn set_wifi_mac_reset(
        &mut self,
        reset: oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset,
    );
    /// What the domain keeps inside the block, for the lowered width `cbw`.
    fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8);
}

impl MacBlockResetPort for SharedRadioRegisters {
    fn set_wifi_mac_reset(
        &mut self,
        reset: oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset,
    ) {
        self.radio_phy_mut().set_wifi_mac_reset(reset);
    }
    fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8) {
        SharedRadioRegisters::write_back_wifi_mac_foreign_words(self, cbw);
    }
}

/// Register transaction of [`RadioDomain::reset_wifi_mac_block`]: the pulse,
/// then the write-back; nothing before the pulse, which would clear it.
pub(crate) fn reset_wifi_mac_block(
    port: &mut impl MacBlockResetPort,
    cbw: u8,
) -> crate::radio::ieee80211::init::MacBlockReset {
    port.set_wifi_mac_reset(oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Asserted);
    port.set_wifi_mac_reset(oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released);
    port.write_back_wifi_mac_foreign_words(cbw);
    crate::radio::ieee80211::init::MacBlockReset::after_pulse()
}

/// Register transaction of [`RadioDomain::reset_stopped_wifi_mac_block`]:
/// the stop's proof taken, the same pulse and write-back, the release's proof
/// beside the block's.
pub(crate) fn reset_stopped_wifi_mac_block(
    port: &mut impl MacBlockResetPort,
    stop: crate::radio::ieee80211::mac::StopRequested,
    cbw: u8,
) -> (
    crate::radio::ieee80211::init::MacBlockReset,
    crate::radio::ieee80211::init::MacStopReset,
) {
    let crate::radio::ieee80211::mac::StopRequested { _private: () } = stop;
    (
        reset_wifi_mac_block(port, cbw),
        crate::radio::ieee80211::init::MacStopReset { _private: () },
    )
}

#[cfg(test)]
mod tests;

use crate::radio::modem::SharedRadioRegisters;

fn modem_error(error: oer_chips_family_radio_power::modem::Error) -> ModemClockError {
    use oer_chips_family_radio_power::modem::Error;
    match error {
        Error::AlreadyEnabled => ModemClockError::AlreadyEnabled,
        Error::NotEnabled => ModemClockError::NotEnabled,
        Error::Rejected => ModemClockError::Rejected,
        Error::Poisoned => ModemClockError::Poisoned,
    }
}
struct ExternalBinding<'a, B: PlatformClockBackend, P> {
    registers: &'a mut SharedRadioRegisters,
    clocks: &'a mut ModemClocks,
    held: &'a mut ModemPlatformClocks<B>,
    platform: P,
}
impl<B: PlatformClockBackend, P>
    oer_chips_family_radio_power::domain::ExternalPort<crate::radio::coex::ExternalCoexConfig>
    for ExternalBinding<'_, B, P>
{
    type Error = crate::radio::coex::ExternalCoexError;
    // The family port's one translation of its flag into the two atoms.
    fn enabled(&mut self, enabled: bool) {
        if enabled {
            self.registers.enable_external_coex();
        } else {
            self.registers.disable_external_coex();
        }
    }
    fn clear_priorities(&mut self, config: crate::radio::coex::ExternalCoexConfig) {
        self.registers.clear_external_coex_priorities(config.role);
    }
    fn disable_clock(&mut self) -> Result<(), Self::Error> {
        self.clocks
            .disable(
                EXTERNAL_COEX_SLOT,
                &mut ModemExecution {
                    port: self.registers.radio_phy_mut(),
                    held: self.held,
                    platform: (),
                },
            )
            .map_err(|error| crate::radio::coex::ExternalCoexError::Clock(modem_error(error)))
    }
}
impl<B: PlatformClockBackend, P: PlatformClockProvider<Backend = B>>
    oer_chips_family_radio_power::domain::ExternalStartPort<crate::radio::coex::ExternalCoexConfig>
    for ExternalBinding<'_, B, &P>
{
    type Delay = crate::radio::modem::values::ExternalCoexGrantDelay;
    fn already_active() -> Self::Error {
        crate::radio::coex::ExternalCoexError::AlreadyActive
    }
    fn grant_delay(
        config: crate::radio::coex::ExternalCoexConfig,
    ) -> Result<Self::Delay, Self::Error> {
        config
            .grant_delay()
            .ok_or(crate::radio::coex::ExternalCoexError::GrantDelayOutOfRange)
    }
    fn enable_clock(&mut self) -> Result<(), Self::Error> {
        self.clocks
            .enable(
                EXTERNAL_COEX_SLOT,
                crate::power::clock::module_dependencies(ModemClockModule::Coexistence),
                &mut ModemExecution {
                    port: self.registers.radio_phy_mut(),
                    held: self.held,
                    platform: self.platform,
                },
            )
            .map_err(|error| crate::radio::coex::ExternalCoexError::Clock(modem_error(error)))
    }
    fn mode(&mut self, config: crate::radio::coex::ExternalCoexConfig) {
        self.registers
            .configure_external_coex_mode(config.role, config.wires);
    }
    fn grant(&mut self, config: crate::radio::coex::ExternalCoexConfig, delay: Self::Delay) {
        self.registers
            .configure_external_coex_grant(delay, config.grant_validity);
    }
    fn priorities(&mut self, config: crate::radio::coex::ExternalCoexConfig) {
        self.registers.publish_external_coex_priorities(
            config.role,
            config.wires,
            config.priorities(),
        );
    }
}
