//! The probe image's verifier session, over the production root only.
//!
//! [`init`], the session's first entry, takes the chip's reset root once
//! (`Root::take`), splits its unstarted owners (`Root::into_runtime`), calls
//! the family adoption driver without a staged handoff, and splits the radio once, and brings Wi-Fi up through its public client path: common
//! power, the module clocks and the running owner. Every later entry
//! borrows the owners it needs from the session ([`with`]); none claims a
//! register again.

use oer_chips_esp32s31_hal::{
    power::ChipClocks,
    radio::domain::{CommonPower, RadioDomain, WifiClient},
    radio::ieee80211::client::{WifiClocksOn, WifiCold},
    system::owner::RadioAccess,
    system::root::Root,
};
use oer_chips_esp32s31_radio_phy::concurrent::ConcurrentPhy;
use oer_portable_memory::ClaimCell;

/// The owners the session's entries borrow.
pub(crate) struct Session {
    pub(crate) calibrated: bool,
    /// The shared radio domain.
    pub(crate) radio: Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
    radio_fault: oer_chips_esp32s31_hal::radio::domain::transaction::FaultHandle,
    /// The running Wi-Fi register owner.
    pub(crate) wifi: RadioAccess,
    /// The receive walker's registers, which no ring takes here.
    pub(crate) walker: oer_chips_esp32s31_hal::radio::ieee80211::registers::WifiRxDmaRegisters,
    /// Wi-Fi's common power, held for the session.
    _power: CommonPower<WifiClient>,
    /// The proof that Wi-Fi's module clocks are on.
    _clocks_on: WifiClocksOn,
    /// The platform clock owner.
    _clocks: ChipClocks,
    /// The settle counter at the adopted CPU rate.
    pub(crate) cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
}

static SESSION: ClaimCell<Session> = ClaimCell::new();

/// The session started.
pub(crate) const STARTED: u32 = 0;
/// The image took the reset root before: the session started already.
pub(crate) const ROOT_TAKEN: u32 = 1;
/// Wi-Fi's common power did not come up.
pub(crate) const WIFI_POWER: u32 = 2;
/// Wi-Fi's module clocks did not come up.
pub(crate) const WIFI_CLOCKS: u32 = 3;
/// The session cell held a session already.
pub(crate) const INSTALLED: u32 = 4;
/// The reset root could not install the platform owners.
pub(crate) const ROOT_INIT: u32 = 5;
/// The PHY's cold registration failed.
pub(crate) const REGISTRATION: u32 = 6;
/// The registration's place was taken: a second calibration.
pub(crate) const REGISTRATION_PLACE: u32 = 7;

/// The PHY's cold registration future, built at its final address: built
/// in the calibration's own state, it was assembled on the radio
/// executor's interrupt stack first and moved, which put its 8 KiB twice on
/// that stack. `StaticPlace` checks its size when the image compiles.
#[cfg(target_arch = "riscv32")]
static REGISTRATION_FUTURE: static_cell::ConstStaticCell<
    oer_portable_memory::StaticPlace<[u128; 544]>,
> = static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());

/// Start the session: [`STARTED`] or why it did not.
pub(crate) fn init(
    identity: impl FnOnce() -> oer_chips_esp32s31_hal::radio::domain::transaction::PhyIdentity,
    report: impl FnOnce(
        oer_chips_esp32s31_hal::radio::domain::transaction::Fault,
    ) -> core::convert::Infallible,
) -> u32 {
    let Some(root) = Root::take() else {
        return ROOT_TAKEN;
    };
    let Ok(platform) = oer_chips_family_boot::adoption::initialize(root.into_runtime()) else {
        return ROOT_INIT;
    };
    let runtime = platform.hardware;
    let transaction_identity = identity();
    let clocks = runtime.clocks;
    let cycles = runtime.cycles;
    let (radio, partitions) = runtime
        .radio
        .split::<ChipClocks, _>(ConcurrentPhy::new(), transaction_identity)
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            match report(fault) {}
        });
    let session = match powered(radio, partitions.wifi, clocks, cycles) {
        Ok(session) => session,
        Err(reason) => return reason,
    };
    install(session)
}

fn powered(
    mut radio: RadioDomain<ChipClocks, ConcurrentPhy>,
    wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    clocks: ChipClocks,
    cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
) -> Result<Session, u32> {
    let (cold, walker) = WifiCold::from_partition(wifi);
    let Ok((powered, power)) = cold.power_up(&mut radio, &clocks) else {
        return Err(WIFI_POWER);
    };
    let Ok((clocked, _first)) = powered.enable_clocks(&mut radio, &clocks) else {
        return Err(WIFI_CLOCKS);
    };
    let (wifi, clocks_on) = clocked.into_running();
    let radio_fault = radio.fault_handle();
    Ok(Session {
        calibrated: false,
        radio: Some(radio),
        radio_fault,
        wifi,
        walker,
        _power: power,
        _clocks_on: clocks_on,
        _clocks: clocks,
        cycles,
    })
}

/// One production cold registration, before the Wi-Fi client enters its clocks.
#[cfg(target_arch = "riscv32")]
#[allow(
    dead_code,
    reason = "the staged companion calls this shared session path"
)]
pub(crate) async fn calibrated(
    hardware: oer_chips_esp32s31_hal::system::root::RadioHardware,
    clocks: ChipClocks,
    identity: oer_chips_esp32s31_radio_phy::PhyCalibrationIdentity,
    timer: &impl oer_portable_cooperate::CooperativeTimer,
    cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
    transaction_identity: oer_chips_esp32s31_hal::radio::domain::transaction::PhyIdentity,
    report: impl FnOnce(
        oer_chips_esp32s31_hal::radio::domain::transaction::Fault,
    ) -> core::convert::Infallible,
) -> u32 {
    use oer_chips_esp32s31_radio_phy::{
        NoopPhyTargetObserver, PhyRegisterConfig, RomShortDelay, register_concurrent_phy,
    };
    let (radio, partitions) = hardware
        .split::<ChipClocks, _>(ConcurrentPhy::new(), transaction_identity)
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            match report(fault) {}
        });
    let Some(place) = REGISTRATION_FUTURE.try_take() else {
        return REGISTRATION_PLACE;
    };
    let mut port = ();
    let timer = oer_chips_esp32s31_radio_phy::executor::wait::PhyTimer::new(timer, cycles);
    let (radio, registration) = place
        .future(|| {
            register_concurrent_phy::<_, (), RomShortDelay, _>(
                &timer,
                radio,
                &mut port,
                &clocks,
                PhyRegisterConfig::new(identity),
                NoopPhyTargetObserver,
            )
        })
        .await;
    if registration.is_err() {
        return REGISTRATION;
    }
    match powered(radio, partitions.wifi, clocks, cycles) {
        Ok(mut session) => {
            session.calibrated = true;
            install(session)
        }
        Err(reason) => reason,
    }
}

/// What [`Session::measurement_clocks`] does to the PHY's calibration clock
/// module.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClockModule {
    Enable,
    Disable,
}

#[cfg(target_arch = "riscv32")]
impl Session {
    /// Enable or disable the PHY's calibration clock module.
    ///
    /// # Errors
    ///
    /// The radio domain's refusal of the module's change.
    pub(crate) fn measurement_clocks(
        &mut self,
        change: ClockModule,
    ) -> Result<(), oer_chips_esp32s31_hal::radio::domain::ModemClockError> {
        use oer_chips_esp32s31_hal::radio::domain::PhyClockModule;
        let radio = self.radio_fault.require_owner(self.radio.as_mut());
        match change {
            ClockModule::Enable => {
                radio.enable_phy_modem_clocks(PhyClockModule::Calibration, &self._clocks)
            }
            ClockModule::Disable => radio.disable_phy_modem_clocks(PhyClockModule::Calibration),
        }
    }
}

pub(crate) fn install(session: Session) -> u32 {
    match SESSION.install(session) {
        Ok(()) => STARTED,
        Err(_) => INSTALLED,
    }
}

/// Run `entry` on the session's owners; `None` when the session did not
/// start.
pub(crate) fn with<R>(entry: impl FnOnce(&mut Session) -> R) -> Option<R> {
    SESSION.lend(entry).ok()
}

impl Session {
    pub(crate) fn radio_mut(&mut self) -> &mut RadioDomain<ChipClocks, ConcurrentPhy> {
        self.radio_fault.require_owner(self.radio.as_mut())
    }
    pub(crate) fn phy<R>(
        &mut self,
        operation: impl FnOnce(
            &mut oer_chips_esp32s31_hal::radio::domain::transaction::PhyHardware,
        ) -> R,
    ) -> R {
        let radio = self.radio_fault.require_owner(self.radio.take());
        let mut transaction =
            radio.detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Maintain);
        let result = operation(transaction.hardware_mut());
        self.radio = Some(transaction.reunite());
        result
    }
}
