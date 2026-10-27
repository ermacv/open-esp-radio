//! The probe image's verifier session, over the production root only.
//!
//! [`init`], the session's first entry, takes the chip's reset root once
//! (`Root::take`), splits it for stage two (`Root::into_runtime`) and the
//! radio once, and brings Wi-Fi up through its public client path: common
//! power, the module clocks and the running owner. Every later entry
//! borrows the owners it needs from the session ([`with`]); none claims a
//! register again.

use oer_esp32s31_hal::{
    domain::{CommonPower, RadioDomain, WifiClient},
    ieee80211::client::{WifiClocksOn, WifiCold},
    owner::RadioAccess,
    power::ChipClocks,
    root::Root,
};
use oer_esp32s31_phy::concurrent::ConcurrentPhy;
use oer_memory::ClaimCell;

/// The owners the session's entries borrow.
pub(crate) struct Session {
    pub(crate) calibrated: bool,
    /// The shared radio domain.
    pub(crate) radio: Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
    radio_fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
    /// The running Wi-Fi register owner.
    pub(crate) wifi: RadioAccess,
    /// Wi-Fi's common power, held for the session.
    _power: CommonPower<WifiClient>,
    /// The proof that Wi-Fi's module clocks are on.
    _clocks_on: WifiClocksOn,
    /// The platform clock owner.
    _clocks: ChipClocks,
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

/// Start the session: [`STARTED`] or why it did not.
pub(crate) fn init(
    identity: impl FnOnce() -> oer_esp32s31_hal::domain::transaction::PhyIdentity,
) -> u32 {
    let Some(root) = Root::take() else {
        return ROOT_TAKEN;
    };
    let Ok(runtime) = root.into_runtime() else {
        return ROOT_INIT;
    };
    let transaction_identity = identity();
    let clocks = runtime.clocks;
    let (radio, partitions) = runtime
        .radio
        .split::<ChipClocks, _>(ConcurrentPhy::new(), transaction_identity);
    let session = match powered(radio, partitions.wifi, clocks) {
        Ok(session) => session,
        Err(reason) => return reason,
    };
    install(session)
}

fn powered(
    mut radio: RadioDomain<ChipClocks, ConcurrentPhy>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    clocks: ChipClocks,
) -> Result<Session, u32> {
    let Ok((powered, power)) = WifiCold::from_partition(wifi).power_up(&mut radio, &clocks) else {
        return Err(WIFI_POWER);
    };
    let Ok(clocked) = powered.enable_clocks(&mut radio, &clocks) else {
        return Err(WIFI_CLOCKS);
    };
    let (wifi, clocks_on) = clocked.into_running();
    let radio_fault = radio.fault_handle();
    Ok(Session {
        calibrated: false,
        radio: Some(radio),
        radio_fault,
        wifi,
        _power: power,
        _clocks_on: clocks_on,
        _clocks: clocks,
    })
}

/// One production cold registration, before the Wi-Fi client enters its clocks.
#[cfg(target_arch = "riscv32")]
#[allow(
    dead_code,
    reason = "the staged companion calls this shared session path"
)]
pub(crate) async fn calibrated(
    hardware: oer_esp32s31_hal::root::RadioHardware,
    clocks: ChipClocks,
    identity: oer_esp32s31_phy::PhyCalibrationIdentity,
    timer: &impl oer_cooperate::CooperativeTimer,
    transaction_identity: oer_esp32s31_hal::domain::transaction::PhyIdentity,
) -> u32 {
    use oer_esp32s31_phy::{
        NoopPhyTargetObserver, PhyRegisterConfig, RomShortDelay, register_concurrent_phy,
    };
    let (radio, partitions) =
        hardware.split::<ChipClocks, _>(ConcurrentPhy::new(), transaction_identity);
    let (radio, registration) = register_concurrent_phy::<_, (), RomShortDelay, _>(
        timer,
        radio,
        &mut (),
        &clocks,
        PhyRegisterConfig::new(identity),
        NoopPhyTargetObserver,
    )
    .await;
    if registration.is_err() {
        return 6;
    }
    match powered(radio, partitions.wifi, clocks) {
        Ok(mut session) => {
            session.calibrated = true;
            install(session)
        }
        Err(reason) => reason,
    }
}

#[cfg(target_arch = "riscv32")]
impl Session {
    pub(crate) fn measurement_clocks(&mut self, enabled: bool) -> bool {
        use oer_esp32s31_hal::domain::PhyClockModule;
        let radio = self.radio_fault.require_owner(self.radio.as_mut());
        if enabled {
            radio
                .enable_phy_modem_clocks(PhyClockModule::Calibration, &self._clocks)
                .is_ok()
        } else {
            radio
                .disable_phy_modem_clocks(PhyClockModule::Calibration)
                .is_ok()
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
        operation: impl FnOnce(&mut oer_esp32s31_hal::domain::transaction::PhyHardware) -> R,
    ) -> R {
        let radio = self.radio_fault.require_owner(self.radio.take());
        let mut transaction =
            radio.detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
        let result = operation(transaction.hardware_mut());
        self.radio = Some(transaction.reunite());
        result
    }
}
