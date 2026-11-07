use oer_chips_esp32s31_radio_ieee80211_driver::mac::{
    edca::*,
    portable::legacy_from_phy_rate,
    tx::{runtime::*, *},
};
use oer_protocols_ieee80211_contracts_port::VifRole;
use oer_protocols_ieee80211_contracts_port::backend::*;
use oer_protocols_ieee80211_wire::{extensions::wmm::parse_wmm_parameter_element, phy::LegacyRate};

const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[test]
fn sta_runtime_policy_owns_peer_facts_and_standard_edca_state() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    assert_eq!(policy.he_bss_color(), 0);
    assert_eq!(
        policy.contention_parameters(VifRole::Station, LegacyTxQueue::BestEffort),
        EdcaContentionParameters::new(3, 4, 10).unwrap()
    );
    assert_eq!(policy.contention_exponent(LegacyTxQueue::BestEffort), 4);

    policy.install_he_bss_color(0xff);
    policy.install_ht_ampdu(HtPeerAmpduParameters::from_capability_byte(0x17));
    assert_eq!(policy.he_bss_color(), 0x3f);
    assert_eq!(
        policy.ht_ampdu(),
        HtPeerAmpduParameters::from_capability_byte(0x17)
    );
    assert_eq!(
        policy.select_backoff(VifRole::Station, LegacyTxQueue::BestEffort, u32::MAX),
        15
    );
}

#[test]
fn vif_tables_are_separate_and_contention_belongs_to_the_ac_queue() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_edca(
            VifRole::AccessPoint,
            oer_protocols_ieee80211_contracts_port::EdcaParameterSet::from_wmm(
                parse_wmm_parameter_element(&STANDARD_WMM).unwrap(),
            ),
        )
        .unwrap();
    let queue = LegacyTxQueue::Voice;
    // The access point's set, whose Voice record requires admission,
    // leaves the station's standard defaults.
    assert!(
        policy
            .access_policy(VifRole::AccessPoint, queue)
            .admission_control_mandatory()
    );
    assert!(
        !policy
            .access_policy(VifRole::Station, queue)
            .admission_control_mandatory()
    );
    // A failed attempt widens the one shared AC queue window.
    let station = policy.contention_exponent(LegacyTxQueue::BestEffort);
    policy.record_retry_failure(VifRole::Station, LegacyTxQueue::BestEffort);
    assert_eq!(
        policy.contention_exponent(LegacyTxQueue::BestEffort),
        station + 1
    );
}

#[test]
fn negotiated_txop_is_bounded_and_medium_ownership_stays_unsupported() {
    let policy = WifiTxRuntimePolicy::vendor_defaults();
    assert_eq!(
        he_txop_limit(94, HeEdcaTxopLimit::DEFAULT),
        Ok(HeEdcaTxopLimit::from_units_32_us(94).unwrap())
    );
    assert_eq!(
        he_txop_limit(94, HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
        Ok(HeEdcaTxopLimit::from_units_32_us(47).unwrap())
    );
    assert_eq!(
        require_ht_txop_support(94),
        Err(WmmTxopUnsupported::HtAggregateDurationBudget { units_32_us: 94 })
    );
    assert_eq!(
        policy.request_hardware_medium(WmmHardwareMediumRequest::RtsCtsProtection),
        Err(WmmTxopUnsupported::RtsCtsProtection)
    );
    assert_eq!(
        policy.request_hardware_medium(WmmHardwareMediumRequest::MultiPpduTxop),
        Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
    );

    assert_eq!(
        he_txop_limit(350, HeEdcaTxopLimit::DEFAULT),
        Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us: 350 })
    );
}

#[test]
fn ordinary_he_retries_walk_the_dot11ax_record() {
    use oer_chips_esp32s31_radio_ieee80211_driver::mac::rx::HeGuardIntervalAndLtf;
    use oer_chips_esp32s31_radio_ieee80211_driver::mac::tx::{HeMcs, HeRate};
    let counters = |failures| RetryCounters {
        mpdu: failures,
        short: 0,
        long: 0,
    };
    let mcs9 = TxPhyRate::He(HeRate::new(
        HeMcs::Mcs9,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
    ));
    assert_eq!(
        oer_protocols_ieee80211_core_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(0),
            &oer_chips_family_radio_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(mcs9)
    );
    assert_eq!(
        oer_protocols_ieee80211_core_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(2),
            &oer_chips_family_radio_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs7,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );
    assert_eq!(
        oer_protocols_ieee80211_core_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(4),
            &oer_chips_family_radio_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(TxPhyRate::Legacy(legacy_from_phy_rate(LegacyRate::Ofdm6M)))
    );
    // An 800-ns rate below MCS9 has no record and keeps its rate.
    let mcs8 = TxPhyRate::He(HeRate::new(HeMcs::Mcs8, HeGuardIntervalAndLtf::OneLtf800Ns));
    assert_eq!(
        oer_protocols_ieee80211_core_upper_mac::retry::select_ordinary_retry_rate(
            mcs8.phy_rate(),
            counters(4),
            &oer_chips_family_radio_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(mcs8)
    );
}

#[test]
fn each_queue_draw_uses_its_head_vif_and_retains_the_complete_record() {
    use oer_protocols_ieee80211_contracts_port::EdcaParameterSet;
    use oer_protocols_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
    let table = |aifsn, min, max, txop, lifetime| {
        EdcaParameterSet::from_tu(
            WmmParameterSet::new(
                0,
                false,
                [WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: oer_portable_wire::U8::new(aifsn).unwrap(),
                    ecw_min: oer_portable_wire::U8::new(min).unwrap(),
                    ecw_max: oer_portable_wire::U8::new(max).unwrap(),
                    txop_limit_units_32_us: txop,
                }; 4],
            ),
            [lifetime; 4],
        )
    };
    let station = table(3, 4, 6, 94, 500);
    let ap = table(7, 2, 3, 350, 100);
    for queue in [
        LegacyTxQueue::Voice,
        LegacyTxQueue::Video,
        LegacyTxQueue::BestEffort,
        LegacyTxQueue::Background,
    ] {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        policy.install_edca(VifRole::Station, station).unwrap();
        assert_eq!(policy.select_backoff(VifRole::Station, queue, u32::MAX), 15);
        policy.record_retry_failure(VifRole::Station, queue);
        // Installing the tail VIF's table cannot change the active head's CW.
        policy.install_edca(VifRole::AccessPoint, ap).unwrap();
        assert_eq!(policy.contention_exponent(queue), 5);
        assert_eq!(policy.select_backoff(VifRole::Station, queue, u32::MAX), 31);
        policy.record_success(VifRole::Station, queue);
        assert_eq!(
            policy.select_backoff(VifRole::AccessPoint, queue, u32::MAX),
            3
        );
        let record = policy.access_policy(VifRole::AccessPoint, queue);
        assert_eq!(record.contention().aifsn(), 7);
        assert_eq!(record.txop_limit_units_32_us(), 350);
        assert_eq!(
            record.msdu_lifetime(),
            oer_portable_time::Duration::from_micros(100 * 1024)
        );
        policy.record_retry_failure(VifRole::AccessPoint, queue);
        assert_eq!(
            policy.select_backoff(VifRole::AccessPoint, queue, u32::MAX),
            7
        );
        policy.reset_terminal_exchange(VifRole::AccessPoint, queue);
        assert_eq!(policy.select_backoff(VifRole::Station, queue, u32::MAX), 15);
    }
}
