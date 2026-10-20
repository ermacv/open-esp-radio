//! The shared radio domain of concurrently running protocols.
//!
//! [`RadioDomain`] owns the shared radio registers together with the shared
//! PHY software state (registration epoch and calibration restore slots).
//! Protocol drivers hold only their own register partitions; the one owner of
//! the domain, the radio system, works on it by `&mut` borrow, so no lock and
//! no lease exists.
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

use oer_esp32s31_pac::{
    BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER, BluetoothTaskRegisters, Ieee802154TaskRegisters,
    RadioPhyRegisters, SharedRadioRegisters, WifiRadioRegisters,
};

pub use crate::clock::{
    BluetoothClient, CommonPower, CommonPowerExitFailure, CommonRadioPowerError, Ieee802154Client,
    RadioClient, RadioClientKind, WifiClient,
};
#[cfg(any(test, oer_probe))]
use crate::coex::PhyGrantProtectError;
use crate::coex::{
    CoexEventId, CoexPti, CoexPtiTable, CoexTimerBank, PHY_GRANT_PROTECT_EVENT, PhyGrantProtect,
};
use crate::coex_participation::CoexParticipants;
pub use crate::coex_participation::{CoexAbsent, Coexisting, ExternalCoexistence};
use crate::power::clock::{
    ModemClockLease, ModemClockModule, ModemClockPlanner, ModemClockPlannerIdentity,
    ModemPlatformClocks, PoisonedModemClockAcquire, PoisonedModemClockRelease, execute_acquire,
    execute_release,
};
pub use crate::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};
use crate::{
    clock::CommonRadioPower,
    owner::{PhyRegistrationEpoch, SharedPhyHal, route},
    phy::restore::PhyRouteState,
};

mod sealed {
    pub trait RadioClientOwner {}
}

const fn client_bit(client: RadioClient) -> u8 {
    match client {
        RadioClient::Wifi => 1,
        RadioClient::Bluetooth => 1 << 1,
        RadioClient::Ieee802154 => 1 << 2,
    }
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
impl sealed::RadioClientOwner for BluetoothTaskRegisters {}
impl RadioClientOwner for BluetoothTaskRegisters {
    type Client = BluetoothClient;
}
impl sealed::RadioClientOwner for Ieee802154TaskRegisters {}
impl RadioClientOwner for Ieee802154TaskRegisters {
    type Client = Ieee802154Client;
}

/// Protocol register owner that may use the shared BTBB baseband.
pub trait BtbbClientOwner: RadioClientOwner {}
impl BtbbClientOwner for BluetoothTaskRegisters {}
impl BtbbClientOwner for Ieee802154TaskRegisters {}

/// Why a client cannot take, leave or use the shared BTBB baseband.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtbbError {
    /// The client already holds the BTBB baseband.
    AlreadyAcquired,
    /// The client does not hold the BTBB baseband.
    NotAcquired,
    /// No PHY registration describes the shared PHY, so no gain parameter can
    /// come from a registered state.
    Unregistered,
    /// The gain was published for an earlier registration, or a later
    /// publication replaced it.
    Stale,
}

/// The BTBB gain byte (`phy_param[0x120]`) one registered PHY state published
/// to the domain ([`BtbbPublisher::publish`]). Only the latest
/// publication of the current registration admits [`RadioDomain::btbb_acquire`];
/// any earlier one is [`BtbbError::Stale`].
#[derive(Debug, Eq, PartialEq)]
pub struct BtbbGain {
    epoch: PhyRegistrationEpoch,
    generation: u32,
    value: u8,
}

impl BtbbGain {
    /// The published byte, as the vendor's `phy_param[0x120]`.
    pub const fn value(&self) -> u8 {
        self.value
    }
}

/// The domain's right to publish a BTBB gain, borrowed beside its attachment
/// ([`RadioDomain::btbb_publisher`]).
pub struct BtbbPublisher<'d> {
    current: Option<PhyRegistrationEpoch>,
    generation: &'d mut u32,
}

impl BtbbPublisher<'_> {
    /// Publish `value`, the BTBB gain byte of the registered PHY state that
    /// `grant` is the right over, replacing every earlier publication. The
    /// registered state's owner publishes its current byte right before a
    /// BTBB acquisition, so the acquisition writes the value the vendor reads
    /// from `phy_param[0x120]` at that moment.
    ///
    /// # Errors
    ///
    /// No registration describes the shared PHY, or `grant` is the right over
    /// an earlier one ([`BtbbError::Stale`]).
    pub fn publish(
        self,
        grant: &crate::owner::PhyRegistrationGrant,
        value: u8,
    ) -> Result<BtbbGain, BtbbError> {
        let epoch = self.current.ok_or(BtbbError::Unregistered)?;
        if grant.epoch() != epoch {
            return Err(BtbbError::Stale);
        }
        *self.generation = self.generation.wrapping_add(1);
        Ok(BtbbGain {
            epoch,
            generation: *self.generation,
            value,
        })
    }
}

/// Whether a BTBB acquisition ran the shared initialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtbbAcquired {
    /// The first holder ran `bt_bb_v2_init_cmplx(1)`.
    Initialized,
    /// Another client already initialized the baseband; no register access.
    Joined,
}

/// Shared registers, the PHY, power and BTBB state that must stay with them,
/// and the upper layer's attachment.
struct SharedRadioState<B: PlatformClockBackend, T> {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower<B>,
    wifi_power_entry: crate::ieee80211::client::WifiPowerEntry,
    btbb_clients: u8,
    /// The generation of the latest BTBB gain publication.
    btbb_gain_generation: u32,
    clocks: ModemClocks,
    /// Platform references of the modem clock planner's enabled
    /// dependencies; a poisoned planner keeps them held.
    modem_platform_clocks: ModemPlatformClocks<B>,
    coex_pti: CoexPtiTable,
    phy_grant_protected: bool,
    /// The low-power timer gate reference while Bluetooth selects its clock.
    bluetooth_low_power_clock: Option<PlatformClockGuard<B>>,
    external_coex: Option<crate::coex::ExternalCoexConfig>,
    coex_participants: CoexParticipants,
    attachment: T,
}

impl<B: PlatformClockBackend, T> SharedRadioState<B, T> {
    /// The platform clock references the radio holds: common power, the
    /// modem clock dependencies (also while poisoned) and the Bluetooth
    /// low-power timer gate.
    fn platform_clock_holds(&self) -> PlatformClockHolds {
        let mut holds = PlatformClockHolds::default();
        self.power.count_platform_clocks(&mut holds);
        self.modem_platform_clocks.count_into(&mut holds);
        holds.count(self.bluetooth_low_power_clock.as_ref());
        holds
    }
}

/// Modem clock planner and the module lease each client holds.
#[allow(
    clippy::large_enum_variant,
    reason = "the domain keeps the allocation-free planner or its poisoned owner inline"
)]
enum ModemClocks {
    Ready {
        planner: ModemClockPlanner,
        leases: [Option<ModemClockLease>; CLOCK_SLOTS],
    },
    /// A transaction failed at a physical edge; the retained owner prevents
    /// any further modem clock change.
    Poisoned {
        _acquire: Option<PoisonedModemClockAcquire>,
        _release: Option<PoisonedModemClockRelease>,
    },
    /// Transient state while one transaction owns the planner.
    InFlight,
}

impl ModemClocks {
    fn new() -> Self {
        Self::Ready {
            planner: ModemClockPlanner::managed(ModemClockPlannerIdentity::new()),
            leases: [const { None }; CLOCK_SLOTS],
        }
    }

    fn any_held(&self) -> bool {
        match self {
            Self::Ready { leases, .. } => leases.iter().any(Option::is_some),
            Self::Poisoned { .. } | Self::InFlight => true,
        }
    }
}

/// Why the Bluetooth low-power timer clock selection cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowPowerClockError {
    /// Bluetooth already selects its low-power timer clock.
    AlreadySelected,
    /// Bluetooth does not select its low-power timer clock.
    NotSelected,
    /// The platform clock owner refused the timer gate reference.
    PlatformClock(PlatformClockError),
}

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
/// use oer_esp32s31_hal::{domain::RadioDomain, power::PlatformClockBackend};
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
    pub(crate) fn new(registers: SharedRadioRegisters, phy: PhyRouteState, attachment: T) -> Self {
        Self {
            state: SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
                wifi_power_entry: crate::ieee80211::client::WifiPowerEntry::default(),
                btbb_clients: 0,
                btbb_gain_generation: 0,
                clocks: ModemClocks::new(),
                modem_platform_clocks: ModemPlatformClocks::default(),
                coex_pti: CoexPtiTable::VENDOR,
                phy_grant_protected: false,
                bluetooth_low_power_clock: None,
                external_coex: None,
                coex_participants: CoexParticipants::default(),
                attachment,
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
        if state.btbb_clients != 0 {
            return Err((Self::from_state(state), RadioDomainReleaseError::BtbbHeld));
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
        if state.bluetooth_low_power_clock.is_some() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::BluetoothLowPowerClockSelected,
            ));
        }
        if state.external_coex.is_some() {
            return Err((
                Self::from_state(state),
                RadioDomainReleaseError::ExternalCoexActive,
            ));
        }
        if let Err(error) = crate::root::check_phy_restore_complete(&state.phy) {
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

    /// Seed an already initialized baseband reference in a native radio model.
    /// Subsequent joins exercise gain admission and ownership without host MMIO;
    /// cold baseband initialization still requires a target test.
    #[cfg(any(test, all(feature = "test-support", not(target_arch = "riscv32"))))]
    #[doc(hidden)]
    pub fn hold_btbb_for_test(&mut self, client: RadioClient) {
        self.state.btbb_clients |= client_bit(client);
    }

    /// Record a programmed PHY grant-protect request without register
    /// access, so a host test or a probe image can check the release alone.
    #[cfg(any(test, oer_probe))]
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
        if let Some(guard) = state.bluetooth_low_power_clock.take() {
            guard.abandon();
        }
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
    /// A client still holds the shared BTBB baseband.
    BtbbHeld,
    /// A client still holds modem clocks, or they are poisoned.
    ModemClocksHeld,
    /// The PHY grant-protect request is still programmed.
    PhyGrantProtectHeld,
    /// Bluetooth still selects its low-power timer clock.
    BluetoothLowPowerClockSelected,
    /// External coexistence still runs.
    ExternalCoexActive,
    /// The radio still holds a platform clock reference no client accounts
    /// for, such as the sources a failed common power sequence took. The
    /// reunion waits, as for the other holds: a retried power-up reuses
    /// those sources, and its client's exit gives them back.
    PlatformClocksHeld,
    /// A PHY calibration still owns a restore obligation.
    Restore(crate::root::RadioPhyReleaseError),
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

    /// The platform clock references the radio holds: common power, the
    /// modem clock dependencies (also while poisoned) and the Bluetooth
    /// low-power timer gate.
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
        let entry = state
            .power
            .prepare_entry::<O::Client>(state.registers.radio_phy_mut())?;
        state
            .power
            .finish_entry(entry, state.registers.radio_phy_mut(), clocks)
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
            .wifi_power_entry
            .enter(&mut state.power, state.registers.radio_phy_mut(), clocks)
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

    /// Whether `client` currently holds the shared BTBB baseband.
    pub fn holds_btbb(&self, client: RadioClient) -> bool {
        self.state().btbb_clients & client_bit(client) != 0
    }

    /// The attachment beside the domain's BTBB gain publisher, borrowed
    /// apart, so the registered PHY state the attachment holds can publish
    /// its gain with its registration's grant ([`BtbbPublisher::publish`]).
    pub fn btbb_publisher(&mut self) -> (&T, BtbbPublisher<'_>) {
        let state = self.state_mut();
        (
            &state.attachment,
            BtbbPublisher {
                current: state.phy.registration_epoch(),
                generation: &mut state.btbb_gain_generation,
            },
        )
    }

    /// Take the shared BTBB baseband as the protocol that owns `owner`.
    ///
    /// This is ESP-IDF's reference-counted `esp_btbb_enable`: only the first
    /// holder runs the `bt_bb_v2_init_cmplx(1)` body with `gain`'s byte, which
    /// also writes the Bluetooth value of the shared TX-on delay; later
    /// holders join without register access. `owner` proves the
    /// Bluetooth/BTBB clocks and resets.
    ///
    /// # Errors
    ///
    /// The client already holds BTBB, no registration describes the shared
    /// PHY, or `gain` is not the latest publication of the current
    /// registration. Each is rejected before any register access.
    pub fn btbb_acquire<O: BtbbClientOwner>(
        &mut self,
        _owner: &O,
        gain: &BtbbGain,
    ) -> Result<BtbbAcquired, BtbbError> {
        let bit = client_bit(O::CLIENT);
        let state = self.state_mut();
        if state.btbb_clients & bit != 0 {
            return Err(BtbbError::AlreadyAcquired);
        }
        let Some(epoch) = state.phy.registration_epoch() else {
            return Err(BtbbError::Unregistered);
        };
        if gain.epoch != epoch || gain.generation != state.btbb_gain_generation {
            return Err(BtbbError::Stale);
        }
        let acquired = if state.btbb_clients == 0 {
            state.registers.initialize_btbb_v2_arg_one(gain.value);
            BtbbAcquired::Initialized
        } else {
            BtbbAcquired::Joined
        };
        state.btbb_clients |= bit;
        Ok(acquired)
    }

    /// Leave the shared BTBB baseband. Like ESP-IDF's `esp_btbb_disable`,
    /// this only drops the reference and performs no register access.
    ///
    /// # Errors
    ///
    /// The client does not hold BTBB.
    pub fn btbb_release<O: BtbbClientOwner>(&mut self, _owner: &O) -> Result<(), BtbbError> {
        let bit = client_bit(O::CLIENT);
        let state = self.state_mut();
        if state.btbb_clients & bit == 0 {
            return Err(BtbbError::NotAcquired);
        }
        state.btbb_clients &= !bit;
        Ok(())
    }

    /// Borrow the shared PHY registers with the route restore state, for a
    /// protocol capability that also borrows its own registers.
    pub(crate) fn phy_parts_mut(
        &mut self,
    ) -> (&mut oer_esp32s31_pac::RadioPhyRegisters, &mut PhyRouteState) {
        let state = self.state_mut();
        (state.registers.radio_phy_mut(), &mut state.phy)
    }

    /// Borrow the shared PHY registers, the route restore state and the
    /// attachment together.
    pub(crate) fn phy_parts_with_attachment(
        &mut self,
    ) -> (
        &mut oer_esp32s31_pac::RadioPhyRegisters,
        &mut PhyRouteState,
        &mut T,
    ) {
        let state = self.state_mut();
        (
            state.registers.radio_phy_mut(),
            &mut state.phy,
            &mut state.attachment,
        )
    }

    /// Borrow the shared PHY for one PHY-layer operation.
    pub fn phy_hal(&mut self) -> SharedPhyHal<'_, route::Shared> {
        let state = self.state_mut();
        SharedPhyHal::new(state.registers.radio_phy_mut(), &mut state.phy)
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
        initialized: bool,
    ) -> Result<(), ModemClockError> {
        match &mut self.state_mut().clocks {
            ModemClocks::Ready { planner, .. } => {
                planner.set_wifi_initialized(initialized);
                Ok(())
            }
            ModemClocks::Poisoned { .. } | ModemClocks::InFlight => Err(ModemClockError::Poisoned),
        }
    }

    /// Whether Bluetooth currently selects its low-power timer clock.
    pub fn bluetooth_low_power_clock_selected(&self) -> bool {
        self.state().bluetooth_low_power_clock.is_some()
    }

    /// Select the main crystal as the Bluetooth low-power timer clock.
    ///
    /// This is ESP-IDF's
    /// `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE, MAIN_XTAL, divider)`
    /// (`components/esp_hw_support/modem/modem_clock.c`, esp-idf
    /// `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`), with the same register
    /// writes in the same order: every source is deselected, the main
    /// crystal is selected, the S31 Bluetooth divider is written and the
    /// timer clock gate is enabled. ESP32-S31 has neither the BLE RTC-timer
    /// nor the Wi-Fi power-clock workaround. The selector fields belong to
    /// Bluetooth alone; the timer gate shares its word with other platform
    /// gates, so its reference comes from `clocks`.
    ///
    /// # Errors
    ///
    /// Bluetooth already selects its clock, or the platform refused the timer
    /// gate after the selector writes.
    pub fn select_bluetooth_low_power_clock(
        &mut self,
        _owner: &BluetoothTaskRegisters,
        clocks: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), LowPowerClockError> {
        let state = self.state_mut();
        if state.bluetooth_low_power_clock.is_some() {
            return Err(LowPowerClockError::AlreadySelected);
        }
        select_bluetooth_low_power_clock(state.registers.radio_phy_mut());
        let timer = clocks
            .acquire(PlatformClock::ModemLowPowerTimer)
            .map_err(LowPowerClockError::PlatformClock)?;
        state.bluetooth_low_power_clock = Some(timer);
        Ok(())
    }

    /// Deselect the Bluetooth low-power timer clock.
    ///
    /// This is `modem_clock_deselect_lp_clock_source(PERIPH_BT_MODULE)` of the
    /// same source: every source is deselected and the timer clock gate
    /// reference is released; the divider keeps its value.
    ///
    /// # Errors
    ///
    /// Bluetooth does not select its clock.
    pub fn deselect_bluetooth_low_power_clock(
        &mut self,
        _owner: &BluetoothTaskRegisters,
    ) -> Result<(), LowPowerClockError> {
        let state = self.state_mut();
        let Some(timer) = state.bluetooth_low_power_clock.take() else {
            return Err(LowPowerClockError::NotSelected);
        };
        deselect_bluetooth_low_power_clock(state.registers.radio_phy_mut());
        timer.release();
        Ok(())
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
        config: crate::coex::ExternalCoexConfig,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(ExternalCoexistence, u32), crate::coex::ExternalCoexError> {
        if self.state().external_coex.is_some() {
            return Err(crate::coex::ExternalCoexError::AlreadyActive);
        }
        let grant_delay = config
            .grant_delay()
            .ok_or(crate::coex::ExternalCoexError::GrantDelayOutOfRange)?;
        self.enable_slot(EXTERNAL_COEX_SLOT, ModemClockModule::Coexistence, platform)
            .map_err(crate::coex::ExternalCoexError::Clock)?;
        let state = self.state_mut();
        let registers = &mut state.registers;
        registers.configure_external_coex_mode(config.role, config.wires);
        registers.configure_external_coex_grant(grant_delay, config.validate_high);
        registers.publish_external_coex_priorities(config.role, config.wires, config.priorities());
        registers.set_external_coex_enabled(true);
        state.external_coex = Some(config);
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
    ) -> Result<u32, crate::coex::ExternalCoexError> {
        let state = self.state_mut();
        let remaining = state.coex_participants.leave_external(external);
        // Only a started external coexistence issues the proof.
        if let Some(config) = state.external_coex.take() {
            state.registers.clear_external_coex_priorities(config.role);
            state.registers.set_external_coex_enabled(false);
        }
        self.disable_slot(EXTERNAL_COEX_SLOT)
            .map_err(crate::coex::ExternalCoexError::Clock)?;
        Ok(remaining)
    }

    /// The running external coexistence, if any.
    pub fn external_coex(&self) -> Option<crate::coex::ExternalCoexConfig> {
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
        let (planner, mut leases) =
            match core::mem::replace(&mut state.clocks, ModemClocks::InFlight) {
                ModemClocks::Ready { planner, leases } => (planner, leases),
                other => {
                    state.clocks = other;
                    return Err(ModemClockError::Poisoned);
                }
            };
        if leases[index].is_some() {
            state.clocks = ModemClocks::Ready { planner, leases };
            return Err(ModemClockError::AlreadyEnabled);
        }
        let prepared = match planner.prepare_acquire(module.dependencies()) {
            Ok(prepared) => prepared,
            Err(failure) => {
                state.clocks = ModemClocks::Ready {
                    planner: failure.into_planner(),
                    leases,
                };
                return Err(ModemClockError::Rejected);
            }
        };
        let phy = state.registers.radio_phy_mut();
        phy.prepare_modem_syscon_clock_map();
        phy.prepare_shared_modem_clock_map();
        match execute_acquire(prepared, phy, &mut state.modem_platform_clocks, platform) {
            Ok((planner, lease)) => {
                leases[index] = Some(lease);
                state.clocks = ModemClocks::Ready { planner, leases };
                Ok(())
            }
            Err(poisoned) => {
                // Remaining leases retire with the poisoned planner epoch.
                state.clocks = ModemClocks::Poisoned {
                    _acquire: Some(poisoned),
                    _release: None,
                };
                Err(ModemClockError::Poisoned)
            }
        }
    }

    fn disable_slot(&mut self, index: usize) -> Result<(), ModemClockError> {
        let state = self.state_mut();
        let (planner, mut leases) =
            match core::mem::replace(&mut state.clocks, ModemClocks::InFlight) {
                ModemClocks::Ready { planner, leases } => (planner, leases),
                other => {
                    state.clocks = other;
                    return Err(ModemClockError::Poisoned);
                }
            };
        let Some(lease) = leases[index].take() else {
            state.clocks = ModemClocks::Ready { planner, leases };
            return Err(ModemClockError::NotEnabled);
        };
        let prepared = match planner.prepare_release(lease) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (planner, lease) = failure.into_owners();
                leases[index] = Some(lease);
                state.clocks = ModemClocks::Ready { planner, leases };
                return Err(ModemClockError::Rejected);
            }
        };
        match execute_release(
            prepared,
            state.registers.radio_phy_mut(),
            &mut state.modem_platform_clocks,
        ) {
            Ok(planner) => {
                state.clocks = ModemClocks::Ready { planner, leases };
                Ok(())
            }
            Err(poisoned) => {
                state.clocks = ModemClocks::Poisoned {
                    _acquire: None,
                    _release: Some(poisoned),
                };
                Err(ModemClockError::Poisoned)
            }
        }
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
    #[cfg(any(test, oer_probe))]
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
    #[cfg(any(test, oer_probe))]
    pub fn acquire_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().acquire()
    }

    /// Withdraw the PHY grant-protect request ([`PhyGrantProtect::release`]).
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
    #[cfg(any(test, oer_probe))]
    pub fn release_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().release()
    }

    /// Whether the PHY grant-protect request is programmed.
    pub fn phy_grant_protected(&self) -> bool {
        self.state().phy_grant_protected
    }

    /// Borrow the coexistence timer bank for one policy transaction.
    pub fn coex_timer_bank(&mut self) -> CoexTimerBank<'_> {
        CoexTimerBank::from_owned(&mut self.state_mut().registers)
    }

    /// Borrow the upper layer's attachment.
    pub fn attachment(&self) -> &T {
        &self.state().attachment
    }

    /// Mutably borrow the upper layer's attachment.
    pub fn attachment_mut(&mut self) -> &mut T {
        &mut self.state_mut().attachment
    }

    /// Borrow the shared PHY together with the attachment, for an upper-layer
    /// operation that updates its state while driving the PHY.
    pub fn phy_hal_with_attachment(&mut self) -> (SharedPhyHal<'_, route::Shared>, &mut T) {
        let state = self.state_mut();
        (
            SharedPhyHal::new(state.registers.radio_phy_mut(), &mut state.phy),
            &mut state.attachment,
        )
    }

    /// Borrow the shared PHY, the attachment and the PHY grant-protect
    /// request together, for PHY maintenance that brackets its hardware
    /// regions with the request.
    pub fn phy_hal_with_attachment_and_grant(
        &mut self,
    ) -> (SharedPhyHal<'_, route::Shared>, &mut T, PhyGrantProtect<'_>) {
        let state = self.state_mut();
        let pti = state.coex_pti.pti(PHY_GRANT_PROTECT_EVENT);
        let (phy, timers) = state.registers.radio_phy_and_coex_timers_mut();
        (
            SharedPhyHal::new(phy, &mut state.phy),
            &mut state.attachment,
            PhyGrantProtect::new(timers, pti, &mut state.phy_grant_protected),
        )
    }

    /// Borrow the shared registers for a protocol read-back that also reads
    /// protocol registers.
    /// Registers of the radio-PHY partition image [`Self::phy_register_image`]
    /// reads.
    pub const PHY_REGISTER_IMAGE_LEN: usize =
        oer_esp32s31_pac::RadioPhyRegisters::REGISTER_IMAGE_LEN;

    /// Registers of the radio-PHY partition image.
    pub const fn phy_register_image_len(&self) -> usize {
        Self::PHY_REGISTER_IMAGE_LEN
    }

    /// Read register `index` of the radio-PHY partition image under this
    /// domain, without effect, for hardware cross-checks of the calibrated
    /// state; `None` past its end. A register whose clock domain is off
    /// may not answer, so a caller reads only indices known to be readable
    /// in the current radio state.
    pub fn phy_register_image(&self, index: usize) -> Option<u32> {
        self.registers().radio_phy().register_image(index)
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

/// Register transaction of [`RadioDomain::select_bluetooth_low_power_clock`].
pub(crate) fn select_bluetooth_low_power_clock(phy: &mut RadioPhyRegisters) {
    phy.select_bluetooth_low_power_timer_main_crystal(BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER);
}

/// Register transaction of [`RadioDomain::deselect_bluetooth_low_power_clock`].
pub(crate) fn deselect_bluetooth_low_power_clock(phy: &mut RadioPhyRegisters) {
    phy.deselect_bluetooth_low_power_timer_sources();
}

#[cfg(test)]
mod tests;
