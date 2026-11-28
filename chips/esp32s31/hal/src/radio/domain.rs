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
    CommonPower, CommonPowerExitFailure, CommonRadioPowerError, RadioClient, RadioClientKind,
    WifiClient,
};
pub use crate::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};
#[cfg(test)]
use crate::radio::coex::PhyGrantProtectError;
pub use crate::radio::coex::participation::{CoexAbsent, Coexisting};
use crate::radio::coex::{CoexEventId, CoexPti, CoexPtiTable};
#[cfg(test)]
use crate::radio::coex::{PHY_GRANT_PROTECT_EVENT, PhyGrantProtect};
use crate::{
    power::radio::CommonRadioPower, radio::phy::restore::PhyRouteState,
    system::owner::PhyRegistrationEpoch,
};

mod lent;
pub use lent::{PhyHardware, PhyLoan};

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

/// Shared registers and the PHY and power state that must stay with them.
struct SharedRadioState<B: PlatformClockBackend> {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower<B>,
    clocks: ModemClocks,
    /// Platform references of the modem clock planner's enabled
    /// dependencies; a poisoned planner keeps them held.
    modem_platform_clocks: ModemPlatformClocks<B>,
    phy_grant_protected: bool,
}

impl<B: PlatformClockBackend> SharedRadioState<B> {
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

/// One modem clock lease slot per protocol client and per PHY-domain module.
const CLOCK_SLOTS: usize = 5;

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

/// The right to the PHY's one state place.
///
/// [`RadioHardware::split`](crate::system::root::RadioHardware::split) makes
/// it once, beside the domain, in
/// [`ProtocolPartitions`](crate::system::root::ProtocolPartitions);
/// [`RadioHardware::reunite`](crate::system::root::RadioHardware::reunite)
/// takes it back. The PHY's owner makes its place from it, so one split has
/// one place.
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::domain::{ChipPhy, PhyStateRight};
/// let _forged = PhyStateRight::<ChipPhy> { _sealed: core::marker::PhantomData };
/// ```
///
/// ```compile_fail,E0277
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<oer_chips_esp32s31_hal::radio::domain::PhyStateRight<oer_chips_esp32s31_hal::radio::domain::ChipPhy>>();
/// ```
#[must_use = "the PHY's place is made from this right, and the reunion takes it back"]
#[derive(Debug)]
pub struct PhyStateRight<P> {
    _sealed: core::marker::PhantomData<P>,
}

/// The brand of this chip's one PHY: its right, its place, its handle and
/// every proof of its RF carry it, so that a proof of one PHY cannot serve
/// another's handle.
#[derive(Debug)]
pub enum ChipPhy {}

impl PhyStateRight<ChipPhy> {
    pub(crate) const fn issued() -> Self {
        Self {
            _sealed: core::marker::PhantomData,
        }
    }

    pub(crate) const fn returned(self) {
        let Self { _sealed: _ } = self;
    }
}

/// The shared radio domain: the shared radio registers, the shared PHY
/// software state and common radio power, owned by value.
///
/// ```compile_fail,E0277
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
pub struct RadioDomain<B: PlatformClockBackend> {
    state: SharedRadioState<B>,
}

impl<B: PlatformClockBackend> RadioDomain<B> {
    /// Place the shared owner and its PHY state in one domain. This performs
    /// no MMIO.
    pub(crate) fn new(
        registers: SharedRadioRegisters,
        phy: PhyRouteState,
        clock_identity: ModemClockPlannerIdentity,
    ) -> Self {
        Self {
            state: SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
                clocks: ModemClocks::new(clock_identity),
                modem_platform_clocks: ModemPlatformClocks::default(),
                phy_grant_protected: false,
            },
        }
    }

    /// What keeps the domain from giving its parts back, or nothing. This
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// A client still holds common power, modem or platform clocks or the
    /// grant-protect request, or a PHY calibration still owns a restore
    /// obligation.
    pub(crate) fn releasable(&self) -> Result<(), RadioDomainReleaseError> {
        let state = &self.state;
        if state.power.any_client() {
            return Err(RadioDomainReleaseError::CommonPowerHeld);
        }
        if state.clocks.any_held() {
            return Err(RadioDomainReleaseError::ModemClocksHeld);
        }
        if state.phy_grant_protected {
            return Err(RadioDomainReleaseError::PhyGrantProtectHeld);
        }
        if let Err(error) = crate::system::root::check_phy_restore_complete(&state.phy) {
            return Err(RadioDomainReleaseError::Restore(error));
        }
        if !state.platform_clock_holds().is_empty() {
            return Err(RadioDomainReleaseError::PlatformClocksHeld);
        }
        Ok(())
    }

    /// The parts, from a domain [`Self::releasable`] found holding nothing.
    pub(crate) fn into_released_parts(self) -> (SharedRadioRegisters, PhyRouteState) {
        (self.state.registers, self.state.phy)
    }

    /// The parts, or the unchanged domain with what keeps it.
    #[cfg(test)]
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "a test's view of the release and its refusal"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> Result<(SharedRadioRegisters, PhyRouteState), (Self, RadioDomainReleaseError)> {
        match self.releasable() {
            Ok(()) => Ok(self.into_released_parts()),
            Err(error) => Err((self, error)),
        }
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
    /// The radio still holds a platform clock reference no client accounts
    /// for, such as the sources a failed common power sequence took. The
    /// reunion waits, as for the other holds: a retried power-up reuses
    /// those sources, and its client's exit gives them back.
    PlatformClocksHeld,
    /// A PHY calibration still owns a restore obligation.
    Restore(crate::system::root::RadioPhyReleaseError),
}

impl<B: PlatformClockBackend> RadioDomain<B> {
    fn state(&self) -> &SharedRadioState<B> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SharedRadioState<B> {
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

    /// Enter client `K` into coexistence. The shared radio's owner applies
    /// the coexistence policy.
    pub fn enter_coex<K: RadioClientKind>(&mut self, absent: CoexAbsent<K>) -> Coexisting<K> {
        absent.enter()
    }

    /// Leave client `K` from coexistence.
    pub fn leave_coex<K: RadioClientKind>(&mut self, coexisting: Coexisting<K>) -> CoexAbsent<K> {
        coexisting.leave()
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
    /// operation that also borrows a protocol's own registers.
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

    /// The coexistence priority of `event`: the vendor's table, which no
    /// code of this tree changes.
    pub fn coex_pti(&self, event: CoexEventId) -> CoexPti {
        CoexPtiTable::VENDOR.pti(event)
    }

    /// Borrow the PHY grant-protect request for one maintenance operation.
    #[cfg(test)]
    pub fn phy_grant_protect(&mut self) -> PhyGrantProtect<'_> {
        let state = self.state_mut();
        let pti = CoexPtiTable::VENDOR.pti(PHY_GRANT_PROTECT_EVENT);
        PhyGrantProtect::new(
            state.registers.coex_grant_timer_mut(),
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
    fn set_wifi_mac_reset(&mut self, reset: oer_chips_esp32s31_pac::dictionaries::Reset);
    /// What the domain keeps inside the block, for the lowered width `cbw`.
    fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8);
}

impl MacBlockResetPort for SharedRadioRegisters {
    fn set_wifi_mac_reset(&mut self, reset: oer_chips_esp32s31_pac::dictionaries::Reset) {
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
    port.set_wifi_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted);
    port.set_wifi_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released);
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
