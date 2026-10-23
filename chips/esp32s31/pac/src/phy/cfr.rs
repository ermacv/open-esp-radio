//! Safe ownership-bound HCCFR and ICCFR register transactions.
//!
//! Legal scalar values and discrete states come from the generated PAC. The
//! separately ordered read-modify-write edges remain identical to the
//! recovered vendor leaves.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
pub use crate::generated::CfrValue;
use crate::generated::{CfrEnableState, CfrForceMode, IccfrGateState};

const fn cfr_enable_state(enabled: bool) -> CfrEnableState {
    if enabled {
        CfrEnableState::Enabled
    } else {
        CfrEnableState::Disabled
    }
}

const fn cfr_force_mode(mode: bool) -> CfrForceMode {
    if mode {
        CfrForceMode::High
    } else {
        CfrForceMode::Low
    }
}

const fn iccfr_gate_state(enabled: bool) -> IccfrGateState {
    if enabled {
        IccfrGateState::Enabled
    } else {
        IccfrGateState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Publish both fields of complete pinned `phy_config_hccfr` in order.
    pub fn configure_hccfr(&mut self, enabled: bool, value: CfrValue) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_hccfr_enable_state(bb, cfr_enable_state(enabled));
        crate::generated::configure_hccfr_value(bb, value);
    }

    /// Apply either complete branch of pinned `phy_iccfr_en`.
    pub fn configure_iccfr_gate(&mut self, enabled: bool) {
        crate::generated::configure_iccfr_gate_state(
            &self.peripherals.phy_baseband_config_oracle,
            iccfr_gate_state(enabled),
        );
    }

    /// Publish all five fields and the tail gate of pinned `phy_force_iccfr`.
    pub fn configure_forced_iccfr(&mut self, mode: bool, enabled: bool, value: CfrValue) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let mode = cfr_force_mode(mode);
        crate::generated::configure_forced_iccfr_mode_high(bb, mode);
        crate::generated::configure_forced_iccfr_enable_state(bb, cfr_enable_state(enabled));
        crate::generated::trigger_forced_iccfr(bb);
        crate::generated::configure_forced_iccfr_mode_low(bb, mode);
        crate::generated::configure_forced_iccfr_value(bb, value);
        self.configure_iccfr_gate(enabled);
    }
}
