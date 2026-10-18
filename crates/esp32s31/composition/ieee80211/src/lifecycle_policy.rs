//! Terminal lifecycle escalation selected by the final composition.
//!
//! Radio drivers classify their failures; only this composition maps a
//! terminal shared-PHY reason to system reset. Radio drivers and the shared
//! PHY contract have no dependency on the SoC reset or watchdog mechanism.

use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

/// Reset the system for a terminal `reason` while the caller still borrows
/// the retained failure frontier, so no owner is moved or dropped before the
/// platform reset. `None` retains the failure without escalation.
pub(crate) fn enforce<E: ?Sized>(_failure: &E, reason: Option<SharedPhyFailStop>) {
    if let Some(reason) = reason {
        let _reason = core::hint::black_box(reason);
        oer_esp32s31_soc_esp_hal::reset_system()
    }
}
