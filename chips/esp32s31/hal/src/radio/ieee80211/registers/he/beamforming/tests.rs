use super::*;

#[test]
fn report_profile_matches_all_three_complete_trc_policy_outputs() {
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            1,
            0x10,
            crate::types::MacHeBeamformingReportDcm::Disabled,
            crate::types::MacHeBeamformingReportErsu::Disabled
        ),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: Bits::constant::<1>(),
            normalized_rate: Bits::constant::<0>(),
            dcm: crate::types::MacHeBeamformingReportDcm::Disabled,
            extended_range_single_user: crate::types::MacHeBeamformingReportErsu::Disabled,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            2,
            0x10,
            crate::types::MacHeBeamformingReportDcm::Enabled,
            crate::types::MacHeBeamformingReportErsu::Enabled
        ),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: Bits::constant::<2>(),
            normalized_rate: Bits::constant::<0>(),
            dcm: crate::types::MacHeBeamformingReportDcm::Enabled,
            extended_range_single_user: crate::types::MacHeBeamformingReportErsu::Enabled,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            0,
            0x0b,
            crate::types::MacHeBeamformingReportDcm::Disabled,
            crate::types::MacHeBeamformingReportErsu::Disabled
        ),
        Ok(MacHeBeamformingReportProfile {
            signal_mode: Bits::constant::<0>(),
            normalized_rate: Bits::constant::<0x0b>(),
            dcm: crate::types::MacHeBeamformingReportDcm::Disabled,
            extended_range_single_user: crate::types::MacHeBeamformingReportErsu::Disabled,
        })
    );
}

#[test]
fn report_profile_rejects_blob_wrap_and_field_truncation() {
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            4,
            0x10,
            crate::types::MacHeBeamformingReportDcm::Disabled,
            crate::types::MacHeBeamformingReportErsu::Disabled
        ),
        Err(MacHeBeamformingReportProfileError::SignalMode(4))
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            1,
            0x0f,
            crate::types::MacHeBeamformingReportDcm::Disabled,
            crate::types::MacHeBeamformingReportErsu::Disabled
        ),
        Err(MacHeBeamformingReportProfileError::RateCode {
            signal_mode: 1,
            rate_code: 0x0f,
        })
    );
    assert_eq!(
        MacHeBeamformingReportProfile::from_hal_arguments(
            0,
            0x20,
            crate::types::MacHeBeamformingReportDcm::Disabled,
            crate::types::MacHeBeamformingReportErsu::Disabled
        ),
        Err(MacHeBeamformingReportProfileError::RateCode {
            signal_mode: 0,
            rate_code: 0x20,
        })
    );
}

#[test]
fn a_report_profile_reaches_all_three_selectors() {
    let mut words = crate::radio::ieee80211::registers::HostWords::filled(0);
    let mut owners = words.owners();
    let profile = MacHeBeamformingReportProfile::from_hal_arguments(
        2,
        0x1c,
        crate::types::MacHeBeamformingReportDcm::Enabled,
        crate::types::MacHeBeamformingReportErsu::Disabled,
    )
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
            (report.qam16_dcm() == crate::types::MacHeBeamformingReportDcm::Enabled),
            (report.qam16_ersu() == crate::types::MacHeBeamformingReportErsu::Enabled),
        ),
        (
            report.qpsk_rate(),
            report.qpsk_signal_mode(),
            (report.qpsk_dcm() == crate::types::MacHeBeamformingReportDcm::Enabled),
            (report.qpsk_ersu() == crate::types::MacHeBeamformingReportErsu::Enabled),
        ),
        (
            report.bpsk_rate(),
            report.bpsk_signal_mode(),
            (report.bpsk_dcm() == crate::types::MacHeBeamformingReportDcm::Enabled),
            (report.bpsk_ersu() == crate::types::MacHeBeamformingReportErsu::Enabled),
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
    let mut words = crate::radio::ieee80211::registers::HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .set_he_ersu_ack_rate_profile(crate::types::MacHeErsuAckRate::ExtendedRange);
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
