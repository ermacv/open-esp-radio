//! Arbiter of the shared radio partitions for concurrently running routes.
//!
//! [`SharedRadio`] owns the shared radio registers together with the shared
//! PHY software state (registration epoch and calibration restore slots).
//! Protocol routes that run at the same time hold only their own register
//! partitions and borrow the shared owner through a [`SharedRadioLease`].
//!
//! Acquisition never blocks: [`SharedRadio::try_acquire`] either grants the
//! unique lease or reports [`SharedRadioBusy`]. A lease may be held across
//! `await` points, so a long calibration or tracking transaction keeps the
//! shared registers for its whole duration. Because acquisition cannot wait,
//! an interrupt handler that tries to acquire cannot deadlock against a task
//! holding the lease; it only observes `Busy`.
//!
//! The arbiter also owns common radio power. Each protocol enters and leaves
//! it through the lease, proving its identity with its own register set; the
//! first client runs the modem/PHY power sequence once and the last restores
//! the cold-power baseline. Refcounted modem clock dependencies are the shared
//! modem clock planner's, and are not part of this membership.
//!
//! Modem clocks are reference-counted per dependency by the shared modem clock
//! planner inside the arbiter: each client enables and disables its module's
//! clocks through the lease, and a client leaving never disables a clock
//! another client still holds. The platform clock provider supplies the
//! upstream 160 MHz source and the analog-I2C master clock.
//!
//! A lease is not a coexistence grant: it serializes register access, not
//! air time. Forgetting a lease with `mem::forget` leaks it: the arbiter
//! stays busy and no later route can reach the shared registers, which is a
//! fail-stop rather than unsound state.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};

use oer_esp32s31_pac::{
    BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER, BluetoothTaskRegisters, Ieee802154TaskRegisters,
    RadioPhyRegisters, SharedRadioRegisters, WifiRadioRegisters,
};

pub use crate::clock::{
    BluetoothClient, CommonPower, CommonPowerExitFailure, CommonRadioPowerError, Ieee802154Client,
    RadioClient, RadioClientKind, WifiClient,
};
use crate::coex::{
    CoexEventId, CoexPti, CoexPtiTable, CoexTimerBank, PHY_GRANT_PROTECT_EVENT, PhyGrantProtect,
    PhyGrantProtectError,
};
use crate::coex_participation::CoexParticipants;
pub use crate::coex_participation::{CoexAbsent, Coexisting, ExternalCoexistence};
use crate::power::clock::{
    ModemClockLease, ModemClockModule, ModemClockPlanner, ModemClockPlannerIdentity,
    ModemPlatformClocks, PoisonedModemClockAcquire, PoisonedModemClockRelease, execute_acquire,
    execute_release,
};
pub use crate::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockError, PlatformClockGuard,
    PlatformClockHolds, PlatformClockProvider,
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

/// Protocol register owner that proves which client calls the arbiter.
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
struct SharedRadioState<T> {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower,
    btbb_clients: u8,
    clocks: ModemClocks,
    /// Platform references of the modem clock planner's enabled
    /// dependencies; a poisoned planner keeps them held.
    modem_platform_clocks: ModemPlatformClocks,
    coex_pti: CoexPtiTable,
    phy_grant_protected: bool,
    /// The low-power timer gate reference while Bluetooth selects its clock.
    bluetooth_low_power_clock: Option<PlatformClockGuard>,
    external_coex: Option<crate::coex::ExternalCoexConfig>,
    coex_participants: CoexParticipants,
    attachment: T,
}

impl<T> SharedRadioState<T> {
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

/// Identity of the one concurrent planner epoch; the radio is a singleton.
static CONCURRENT_CLOCK_IDENTITY: ModemClockPlannerIdentity = ModemClockPlannerIdentity::new();

/// Modem clock planner and the module lease each client holds.
#[allow(
    clippy::large_enum_variant,
    reason = "the arbiter keeps the allocation-free planner or its poisoned owner inline"
)]
enum ModemClocks {
    Ready {
        planner: ModemClockPlanner<'static>,
        leases: [Option<ModemClockLease<'static>>; CLOCK_SLOTS],
    },
    /// A transaction failed at a physical edge; the retained owner prevents
    /// any further modem clock change.
    Poisoned {
        _acquire: Option<PoisonedModemClockAcquire<'static>>,
        _release: Option<PoisonedModemClockRelease<'static, 'static>>,
    },
    /// Transient state while one transaction owns the planner.
    InFlight,
}

impl ModemClocks {
    const fn new() -> Self {
        Self::Ready {
            planner: ModemClockPlanner::managed(&CONCURRENT_CLOCK_IDENTITY),
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

/// One lease slot per protocol client, per PHY-domain module and for the
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

/// Unique arbiter of the shared radio partitions.
///
/// ```compile_fail
/// use oer_esp32s31_hal::shared_radio::SharedRadio;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<SharedRadio>();
/// ```
///
/// `T` is state an upper layer keeps under the same arbitration, such as the
/// PHY layer's registered domain. The HAL never interprets it; a lease lends
/// it together with the shared registers.
#[must_use = "dropping the shared radio arbiter permanently loses the shared registers"]
pub struct SharedRadio<T = ()> {
    held: AtomicBool,
    state: UnsafeCell<SharedRadioState<T>>,
}

// SAFETY: `state` is dereferenced only through a `SharedRadioLease`, and at
// most one lease exists at a time: `try_acquire` grants one only after
// atomically changing `held` from false to true, and the lease clears it on
// drop after its last access. The shared registers are safe to move between
// execution contexts, and the attachment is `Send`, so granting exclusive
// access from any context is sound.
#[allow(unsafe_code)]
unsafe impl<T: Send> Sync for SharedRadio<T> where SharedRadioRegisters: Send {}

impl<T> SharedRadio<T> {
    /// Place the shared owner, its PHY state and `attachment` under
    /// arbitration. This performs no MMIO.
    pub(crate) fn new(registers: SharedRadioRegisters, phy: PhyRouteState, attachment: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            state: UnsafeCell::new(SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
                btbb_clients: 0,
                clocks: ModemClocks::new(),
                modem_platform_clocks: ModemPlatformClocks::default(),
                coex_pti: CoexPtiTable::VENDOR,
                phy_grant_protected: false,
                bluetooth_low_power_clock: None,
                external_coex: None,
                coex_participants: CoexParticipants::default(),
                attachment,
            }),
        }
    }

    /// Take the unique lease, or report that another holder has it.
    ///
    /// # Errors
    ///
    /// [`SharedRadioBusy`] while another lease is alive or was forgotten.
    pub fn try_acquire(&self) -> Result<SharedRadioLease<'_, T>, SharedRadioBusy> {
        self.held
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| SharedRadioLease { radio: self })
            .map_err(|_| SharedRadioBusy)
    }

    /// Take the lease of an arbiter this caller borrows uniquely, for a
    /// validation image whose memory model admits no atomic access to it.
    ///
    /// The unique borrow proves that no lease is alive, so the flag is set
    /// without an atomic exchange; dropping the lease releases it normally.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn lease_for_validation(&mut self) -> SharedRadioLease<'_, T> {
        *self.held.get_mut() = true;
        SharedRadioLease { radio: self }
    }

    /// Whether a lease is currently held.
    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::Relaxed)
    }

    /// Leave arbitration. Consuming the arbiter proves no lease is alive.
    ///
    /// # Errors
    ///
    /// Returns the unchanged arbiter while a client still holds common power
    /// or a PHY calibration still owns a restore obligation.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the parts, or the unchanged arbiter, are returned exactly as held"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> Result<(SharedRadioRegisters, PhyRouteState, T), (Self, SharedRadioReleaseError)> {
        let state = self.state.into_inner();
        if state.power.any_client() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::CommonPowerHeld,
            ));
        }
        if state.btbb_clients != 0 {
            return Err((Self::from_state(state), SharedRadioReleaseError::BtbbHeld));
        }
        if state.clocks.any_held() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::ModemClocksHeld,
            ));
        }
        if state.phy_grant_protected {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::PhyGrantProtectHeld,
            ));
        }
        if state.bluetooth_low_power_clock.is_some() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::BluetoothLowPowerClockSelected,
            ));
        }
        if state.external_coex.is_some() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::ExternalCoexActive,
            ));
        }
        if let Err(error) = crate::root::check_phy_restore_complete(&state.phy) {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::Restore(error),
            ));
        }
        if !state.platform_clock_holds().is_empty() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::PlatformClocksHeld,
            ));
        }
        Ok((state.registers, state.phy, state.attachment))
    }

    fn from_state(state: SharedRadioState<T>) -> Self {
        Self {
            held: AtomicBool::new(false),
            state: UnsafeCell::new(state),
        }
    }

    #[cfg(test)]
    pub(crate) fn phy_state_mut_for_test(&mut self) -> &mut PhyRouteState {
        &mut self.state.get_mut().phy
    }

    #[cfg(test)]
    pub(crate) fn hold_btbb_for_test(&mut self, client: RadioClient) {
        self.state.get_mut().btbb_clients |= client_bit(client);
    }

    /// Record a BTBB reference of `client` without register access, for
    /// ownership tests of upper layers over an isolated validation root.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn hold_btbb_for_validation(&mut self, client: RadioClient) {
        self.state.get_mut().btbb_clients |= client_bit(client);
    }

    /// Record a programmed PHY grant-protect request without register
    /// access, so a validation image can compare the release alone.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn hold_phy_grant_protect_for_validation(&mut self) {
        self.state.get_mut().phy_grant_protected = true;
    }

    /// Keep every platform clock reference the radio holds on until a chip
    /// reset, and forget them: the explicit end of a terminal validation
    /// image or test that drops its shared radio. Each guard ends with
    /// `abandon`, so none is dropped.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn abandon_platform_clocks(&mut self) {
        let state = self.state.get_mut();
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
        platform: &impl PlatformClockProvider,
    ) {
        self.state
            .get_mut()
            .power
            .hold_platform_clock_for_test(clock, platform);
    }

    #[cfg(test)]
    pub(crate) fn hold_common_power_for_test(&mut self, client: RadioClient) {
        self.state.get_mut().power.hold_for_test(client);
    }
}

/// Why the shared radio cannot leave arbitration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedRadioReleaseError {
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

/// Another holder owns the shared radio lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedRadioBusy;

/// Unique, scoped access to the shared radio partitions.
#[must_use = "a lease blocks every other route until it is dropped"]
pub struct SharedRadioLease<'radio, T = ()> {
    radio: &'radio SharedRadio<T>,
}

impl<T> SharedRadioLease<'_, T> {
    fn state(&self) -> &SharedRadioState<T> {
        // SAFETY: this lease is the unique holder (see `SharedRadio`'s `Sync`
        // proof), so no mutable reference to the state exists elsewhere.
        #[allow(unsafe_code)]
        unsafe {
            &*self.radio.state.get()
        }
    }

    fn state_mut(&mut self) -> &mut SharedRadioState<T> {
        // SAFETY: this lease is the unique holder and `&mut self` prevents a
        // second borrow through it, so the reference is exclusive.
        #[allow(unsafe_code)]
        unsafe {
            &mut *self.radio.state.get()
        }
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
    /// Only the first client runs the modem/PHY power sequence; it pulses the
    /// Wi-Fi baseband and MAC resets, so it never runs while another client
    /// holds power.
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
        clocks: &impl PlatformClockProvider,
    ) -> Result<CommonPower<O::Client>, CommonRadioPowerError> {
        let state = self.state_mut();
        state
            .power
            .enter::<O::Client>(state.registers.radio_phy_mut(), clocks)
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

    /// Take the shared BTBB baseband as the protocol that owns `owner`.
    ///
    /// This is ESP-IDF's reference-counted `esp_btbb_enable`: only the first
    /// holder runs the `bt_bb_v2_init_cmplx(1)` body, which also writes the
    /// Bluetooth value of the shared TX-on delay; later holders join without
    /// register access.
    ///
    /// # Errors
    ///
    /// The client already holds BTBB, or no registration describes the
    /// shared PHY. Both are rejected before any register access.
    ///
    /// # Safety
    ///
    /// The Bluetooth/BTBB clocks and resets must be active, the shared PHY
    /// registration must be complete, and `gain_parameter` must be the byte at
    /// offset `0x120` of that registration's `phy_param` state.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature carries the common-PHY and clock prerequisites"
    )]
    pub unsafe fn btbb_acquire<O: BtbbClientOwner>(
        &mut self,
        _owner: &O,
        gain_parameter: u8,
    ) -> Result<BtbbAcquired, BtbbError> {
        let bit = client_bit(O::CLIENT);
        let state = self.state_mut();
        if state.btbb_clients & bit != 0 {
            return Err(BtbbError::AlreadyAcquired);
        }
        if state.phy.registration_epoch().is_none() {
            return Err(BtbbError::Unregistered);
        }
        let acquired = if state.btbb_clients == 0 {
            state.registers.initialize_btbb_v2_arg_one(gain_parameter);
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
        platform: &impl PlatformClockProvider,
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
        clocks: &impl PlatformClockProvider,
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
        platform: &impl PlatformClockProvider,
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
        platform: &impl PlatformClockProvider,
    ) -> Result<(ExternalCoexistence, u32), crate::coex::ExternalCoexError> {
        if self.state().external_coex.is_some() {
            return Err(crate::coex::ExternalCoexError::AlreadyActive);
        }
        self.enable_slot(EXTERNAL_COEX_SLOT, ModemClockModule::Coexistence, platform)
            .map_err(crate::coex::ExternalCoexError::Clock)?;
        let state = self.state_mut();
        let registers = &mut state.registers;
        registers.configure_external_coex_mode(config.role, config.wires);
        registers.configure_external_coex_grant(config.grant_delay_us, config.validate_high);
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
        platform: &impl PlatformClockProvider,
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

    /// The current priority of an IEEE 802.15.4 coexistence level, as
    /// `coex_ieee802154_pti_get` reads it.
    pub fn coex_ieee802154_pti(&self, level: crate::coex::Ieee802154CoexLevel) -> CoexPti {
        self.state().coex_pti.ieee802154_pti(level)
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
    pub fn acquire_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().acquire()
    }

    /// Withdraw the PHY grant-protect request ([`PhyGrantProtect::release`]).
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
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
    /// lease, without effect, for hardware cross-checks of the calibrated
    /// state; `None` past its end. A register whose clock domain is off
    /// may not answer, so a caller reads only indices known to be readable
    /// in the current radio state.
    pub fn phy_register_image(&self, index: usize) -> Option<u32> {
        self.registers().radio_phy().register_image(index)
    }

    /// Registers of the analog register image [`Self::phy_analog_image`]
    /// reads.
    pub const PHY_ANALOG_IMAGE_LEN: usize = crate::phy::i2c::PhyI2cAddress::IMAGE_LEN;

    /// Registers of the analog register image.
    pub const fn phy_analog_image_len(&self) -> usize {
        Self::PHY_ANALOG_IMAGE_LEN
    }

    /// Read analog register `index` of the analog register image under
    /// this lease, for hardware cross-checks of the calibrated analog state;
    /// `None` past its end. The read is one analog-I2C read command, which
    /// selects the block's host and read mask as every production read
    /// does; the busy host is polled at most `maximum_polls` times.
    pub fn phy_analog_image(
        &mut self,
        index: usize,
        maximum_polls: u32,
    ) -> Option<Result<u8, crate::phy::i2c::PhyI2cAccessError>> {
        let address = crate::phy::i2c::PhyI2cAddress::image(index)?;
        let phy = self.registers_mut().radio_phy_mut();
        let mut polls = 0;
        while let Err(busy) = phy.try_start_phy_i2c_read(address) {
            polls += 1;
            if polls >= maximum_polls {
                return Some(Err(busy));
            }
        }
        loop {
            match phy.try_finish_phy_i2c_read(address) {
                Ok(value) => return Some(Ok(value)),
                Err(busy) => {
                    polls += 1;
                    if polls >= maximum_polls {
                        return Some(Err(busy));
                    }
                }
            }
        }
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

/// Register transaction of [`SharedRadioLease::select_bluetooth_low_power_clock`].
pub(crate) fn select_bluetooth_low_power_clock(phy: &mut RadioPhyRegisters) {
    phy.select_bluetooth_low_power_timer_main_crystal(BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER);
}

/// Register transaction of [`SharedRadioLease::deselect_bluetooth_low_power_clock`].
pub(crate) fn deselect_bluetooth_low_power_clock(phy: &mut RadioPhyRegisters) {
    phy.deselect_bluetooth_low_power_timer_sources();
}

impl<T> Drop for SharedRadioLease<'_, T> {
    fn drop(&mut self) {
        self.radio.held.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
