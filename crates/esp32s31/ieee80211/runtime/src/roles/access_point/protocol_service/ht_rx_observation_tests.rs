
use super::*;

#[test]
fn admitted_data_pm_state_comes_from_the_validated_header_bit() {
    assert_eq!(
        admitted_ap_data_power_state(0x4188),
        ApPeerPowerState::Active
    );
    assert_eq!(
        admitted_ap_data_power_state(0x5188),
        ApPeerPowerState::Sleeping
    );
}

#[test]
fn ap_observation_separates_valid_ht40_mcs32_from_width_mismatch() {
    let mut observation = AccessPointControlObservation::default();
    observe_ht_rx_data_frame(
        &mut observation,
        HtSignal {
            mcs: 32,
            channel_width_mhz: 40,
            aggregation: false,
            short_guard_interval: false,
        },
    );
    observe_ht_rx_data_frame(
        &mut observation,
        HtSignal {
            mcs: 32,
            channel_width_mhz: 20,
            aggregation: true,
            short_guard_interval: false,
        },
    );
    observe_ht_rx_data_frame(
        &mut observation,
        HtSignal {
            mcs: 7,
            channel_width_mhz: 40,
            aggregation: true,
            short_guard_interval: true,
        },
    );

    assert_eq!(observation.rx_ht_data_frames, 3);
    assert_eq!(observation.rx_ht_mpdus_with_aggregation_bit, 2);
    assert_eq!(observation.rx_ht40_mcs32_frames, 1);
    assert_eq!(observation.rx_ht_mcs32_width_mismatches, 1);
    assert_eq!(observation.rx_ht40_mcs_frames[7], 1);
    assert_eq!(observation.rx_ht40_long_gi_frames, 0);
    assert_eq!(observation.rx_ht40_short_gi_frames, 1);
}

#[test]
fn ap_entropy_is_consumed_only_for_a_fresh_wpa2_association() {
    use core::cell::Cell;

    let association = ApManagementRequest::Association {
        peer: [2, 0, 0, 0, 0, 1],
        security: oer_ieee80211_mac::ap::ApAssociationSecurityObservation {
            privacy: true,
            rsn_ie: Some(&[1]),
            rsn_ie_count: 1,
            rsnxe: None,
            rsnxe_count: 0,
            legacy_wpa_present: false,
            malformed_elements: false,
        },
        maximum_legacy_rate_500kbps: 108,
        ht_capabilities: None,
        qos_supported: true,
        short_preamble: true,
    };
    let expected = ([0xa5; 32], 0x1234_5678_9abc_def0);
    let calls = Cell::new(0);
    let mut source = || {
        calls.set(calls.get() + 1);
        expected
    };

    assert_eq!(
        ap_security_material_for_management(
            LinkProtection::Ccmp,
            Some(association),
            Some(ApPeerPhase::Authenticated),
            &mut source,
        ),
        expected
    );
    assert_eq!(calls.get(), 1);

    for (mode, request, phase) in [
        (
            LinkProtection::Ccmp,
            Some(association),
            Some(ApPeerPhase::Securing),
        ),
        (
            LinkProtection::Ccmp,
            None,
            Some(ApPeerPhase::Authenticated),
        ),
        (
            LinkProtection::Open,
            Some(association),
            Some(ApPeerPhase::Authenticated),
        ),
    ] {
        assert_eq!(
            ap_security_material_for_management(mode, request, phase, &mut source),
            ([0; 32], 0)
        );
    }
    assert_eq!(calls.get(), 1);
}
