use super::*;

#[test]
fn report_profile_matches_all_three_complete_trc_policy_outputs() {
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(1, 0x10, false, false),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: 1,
            normalized_rate: 0,
            dcm: false,
            extended_range_single_user: false,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(2, 0x10, true, true),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: 2,
            normalized_rate: 0,
            dcm: true,
            extended_range_single_user: true,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(0, 0x0b, false, false),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: 0,
            normalized_rate: 0x0b,
            dcm: false,
            extended_range_single_user: false,
        })
    );
}

#[test]
fn report_profile_rejects_blob_wrap_and_field_truncation() {
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(4, 0x10, false, false),
        Err(MacHeBeamformingReportProfileError::SignalMode(4))
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(1, 0x0f, false, false),
        Err(MacHeBeamformingReportProfileError::RateCode {
            signal_mode: 1,
            rate_code: 0x0f,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(0, 0x20, false, false),
        Err(MacHeBeamformingReportProfileError::RateCode {
            signal_mode: 0,
            rate_code: 0x20,
        })
    );
}
