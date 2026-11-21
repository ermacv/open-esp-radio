//! A caller above the HAL names every generated value its API takes through
//! the HAL alone: this crate names no PAC path.

use oer_chips_esp32s31_hal::types::{
    MacDsssPreamble, MacHeBeamformingReportDcm, MacHeBeamformingReportErsu,
    MacHeBeamformingReportProfile, MacLegacyRate, MacPlcpRate, MacRtsRate,
};

#[test]
fn a_beamforming_profile_takes_and_returns_the_published_names() {
    let profile = MacHeBeamformingReportProfile::from_hal_arguments(
        0,
        3,
        MacHeBeamformingReportDcm::Enabled,
        MacHeBeamformingReportErsu::Disabled,
    )
    .expect("mode zero takes the rate code as it is");
    assert_eq!(profile.dcm(), MacHeBeamformingReportDcm::Enabled);
    assert_eq!(
        profile.extended_range_single_user(),
        MacHeBeamformingReportErsu::Disabled
    );
}

#[test]
fn a_legacy_rate_gives_its_published_plcp_and_rts_rates() {
    let rate = MacLegacyRate::Dsss2M(MacDsssPreamble::Short);
    assert_eq!(rate.plcp_rate(), MacPlcpRate::Dsss2mShort);
    assert_eq!(rate.rts_rate(), MacRtsRate::Dsss2mShort);
}
