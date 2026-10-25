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
    owner::{MacInterruptSetup, RadioAccess},
    power::ChipClocks,
    root::Root,
};
use oer_memory::ClaimCell;

/// The owners the session's entries borrow.
pub(crate) struct Session {
    /// The shared radio domain.
    pub(crate) radio: RadioDomain<ChipClocks>,
    /// The running Wi-Fi register owner.
    pub(crate) wifi: RadioAccess,
    /// The Wi-Fi MAC's interrupt banks, task side.
    pub(crate) interrupts: MacInterruptSetup,
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

/// Start the session: [`STARTED`] or why it did not.
pub(crate) fn init() -> u32 {
    let Some(root) = Root::take() else {
        return ROOT_TAKEN;
    };
    let runtime = root.into_runtime();
    let clocks = runtime.clocks;
    let (mut radio, partitions) = runtime.radio.split::<ChipClocks, _>(());
    let Ok((powered, power)) =
        WifiCold::from_partition(partitions.wifi).power_up(&mut radio, &clocks)
    else {
        return WIFI_POWER;
    };
    let Ok(clocked) = powered.enable_clocks(&mut radio, &clocks) else {
        return WIFI_CLOCKS;
    };
    let (wifi, interrupts, clocks_on) = clocked.into_running();
    let session = Session {
        radio,
        wifi,
        interrupts,
        _power: power,
        _clocks_on: clocks_on,
        _clocks: clocks,
    };
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
