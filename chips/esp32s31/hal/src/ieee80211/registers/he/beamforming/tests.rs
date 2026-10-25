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

#[test]
fn a_report_profile_reaches_all_three_selectors() {
    let mut words = crate::ieee80211::registers::HostWords::filled(0);
    let mut owners = words.owners();
    let profile = MacHeBeamformingReportProfile::from_hal_arguments(2, 0x1c, true, false)
        .expect("a recovered rate");
    owners.registers.set_he_beamforming_report_profile(profile);
    let report = owners
        .registers
        .mac
        .wifi_mac_he_init_prefix
        .read_bf_report_rate();
    for (rate, signal_mode, dcm, ersu) in [
        (
            report.qam16_rate(),
            report.qam16_signal_mode(),
            report.qam16_dcm(),
            report.qam16_ersu(),
        ),
        (
            report.qpsk_rate(),
            report.qpsk_signal_mode(),
            report.qpsk_dcm(),
            report.qpsk_ersu(),
        ),
        (
            report.bpsk_rate(),
            report.bpsk_signal_mode(),
            report.bpsk_dcm(),
            report.bpsk_ersu(),
        ),
    ] {
        assert_eq!(
            (rate.get(), signal_mode.get(), dcm, ersu),
            (2, 2, true, false)
        );
    }
}

#[test]
fn an_ack_rate_profile_writes_its_byte_four_times() {
    let mut words = crate::ieee80211::registers::HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .set_he_ersu_ack_rate_profile(MacHeErSuAckRateProfile::ExtendedRange);
    let ack = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_ersu_ack_rate();
    assert_eq!(
        [ack.rate_0(), ack.rate_1(), ack.rate_2(), ack.rate_3()],
        [0xa0; 4]
    );
}
