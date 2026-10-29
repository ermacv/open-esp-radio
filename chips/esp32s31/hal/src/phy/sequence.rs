//! PHY operations spanning physical blocks, with host-testable placements.
#![forbid(unsafe_code)]
use super::{
    agc::ops as agc,
    baseband::ops as bb,
    frequency::ops as frequency,
    memory::ops as memory,
    power_detector::platform,
    registers::{RadioPhyParts, RadioPhyRegisters, RadioPlacement},
    restore::{PhyRouteState, TxDcPwdetLifecycleError},
};
use crate::modem::{
    ops as modem,
    values::{WifiBaseband40MhzState, WifiBasebandAgcUpdateMode},
};

// SOURCE: ROM_REV0_PHY_AGC. The system update follows all fourteen AGC edges.
pub(crate) fn update_agc<P: RadioPlacement>(parts: &mut RadioPhyParts<P>) {
    agc::update_baseband_registers(&mut parts.phy_agc_oracle);
    modem::set_wifi_baseband_agc_update_mode(
        &mut parts.modem_syscon_radio,
        WifiBasebandAgcUpdateMode::RegisterUpdatesEnabled,
    );
}
pub(crate) fn configure_rxiq_root_correction<P: RadioPlacement>(
    parts: &mut RadioPhyParts<P>,
    begin: bool,
) {
    bb::configure_rxiq_root_correction(
        &mut parts.phy_baseband_config_oracle,
        &mut parts.phy_pbus,
        begin,
    );
}
// SOURCE: ROM_REV0_PHY_FE_REG_INIT. The table-base update stays between prefix and suffix.
pub(crate) fn initialize_front_end<P: RadioPlacement>(parts: &mut RadioPhyParts<P>) {
    const TABLE_MEMORY_BASE_INDEX: u8 = 0xa0;
    bb::initialize_front_end_prefix(&mut parts.phy_baseband_config_oracle, &mut parts.phy_pbus);
    memory::configure_table_memory_base_index(&mut parts.phy_clock_oracle, TABLE_MEMORY_BASE_INDEX);
    bb::initialize_front_end_suffix(&mut parts.phy_baseband_config_oracle);
}
// SOURCE: complete rev0 ROM phy_bb_reg_init.
pub(crate) fn initialize_baseband<P: RadioPlacement>(parts: &mut RadioPhyParts<P>) {
    bb::initialize_baseband_prefix(&mut parts.phy_baseband_config_oracle);
    frequency::initialize_nrx_baseband(&mut parts.phy_frequency_channel_oracle);
    bb::initialize_baseband_middle(&mut parts.phy_baseband_config_oracle);
    modem::set_wifi_baseband_agc_update_mode(
        &mut parts.modem_syscon_radio,
        WifiBasebandAgcUpdateMode::Initialization,
    );
    bb::initialize_baseband_tail(&mut parts.phy_baseband_config_oracle);
}
// SOURCE: complete rev0 ROM phy_bb_bss_cbw40.
pub(crate) fn configure_bss_cbw<P: RadioPlacement>(parts: &mut RadioPhyParts<P>, cbw: u8) {
    frequency::configure_bss_cbw_prefix(&mut parts.phy_frequency_channel_oracle, cbw);
    modem::set_wifi_baseband_40mhz_state(
        &mut parts.modem_syscon_radio,
        if cbw != 0 {
            WifiBaseband40MhzState::Enabled
        } else {
            WifiBaseband40MhzState::Disabled
        },
    );
    frequency::configure_bss_cbw_suffix(&mut parts.phy_frequency_channel_oracle, cbw);
}
pub(crate) fn observe_software_frequency_boundary<P: RadioPlacement>(phy: &RadioPhyRegisters<P>) {
    let _ = phy.sample_sdm_deadline_counter();
    frequency::observe_software_frequency_boundary(&phy.peripherals.phy_frequency_channel_oracle);
}
// SOURCE: ROM_REV0_PHY_POWER_DETECTOR. Refuse an overlapping restore before any MMIO.
pub(crate) fn initialize_power_detector<P: RadioPlacement>(
    (phy, restore): (&mut RadioPhyRegisters<P>, &mut PhyRouteState),
) -> Result<(), TxDcPwdetLifecycleError> {
    if restore.txdc_pending() {
        return Err(TxDcPwdetLifecycleError::RestorePending);
    }
    bb::initialize_power_detector_registers(&mut phy.peripherals.phy_baseband_config_oracle);
    platform::select_circuit_initialization_mode(&mut phy.peripherals.lp_aon_clkrst);
    Ok(())
}
pub(crate) fn enable_power_detector<P: RadioPlacement>(parts: &mut RadioPhyParts<P>) {
    bb::configure_power_detector_enabled(&mut parts.phy_baseband_config_oracle);
    platform::select_circuit_initialization_mode(&mut parts.lp_aon_clkrst);
}
pub(crate) fn configure_background_power_detector<P: RadioPlacement>(parts: &mut RadioPhyParts<P>) {
    enable_power_detector(parts);
    bb::enable_power_detector_background_control(&mut parts.phy_baseband_config_oracle);
}

pub(crate) fn prepare_txdc<P: RadioPlacement>(
    (phy, restore): (&mut RadioPhyRegisters<P>, &mut PhyRouteState),
) -> Result<(), super::restore::TxDcPwdetPrepareError> {
    restore.prepare_txdc_with(
        &mut phy.peripherals.phy_baseband_config_oracle,
        bb::capture_txdc_power_detector,
        bb::apply_txdc_power_detector_calibration,
    )
}
pub(crate) fn restore_txdc<P: RadioPlacement>(
    (phy, restore): (&mut RadioPhyRegisters<P>, &mut PhyRouteState),
) -> Result<(), super::restore::TxDcPwdetRestoreError> {
    restore.restore_txdc_with(|fields| {
        bb::restore_txdc_power_detector(&mut phy.peripherals.phy_baseband_config_oracle, fields)
    })
}
