use super::*;
use crate::rate::schedule::{RateScheduleKind, RateScheduleRef};
use crate::tx::test_support::model_completion;
use oer_espressif_ieee80211_policy::rate_control::{
    DEFAULT_CONTROL_SCHEDULE, HeLowMetricReportFeatures, StaLinkMetric,
    StaRateControlAssociationInput, StaRateControlPhy,
};

fn association(phy: StaRateControlPhy, p2p: bool) -> StaRateControlAssociation {
    StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy,
        link_metric: StaLinkMetric::from_estimator(70),
        p2p,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    })
}

const HE20_POLICY: StaTxRatePolicy = StaTxRatePolicy {
    association_phy: PhyMode::He20,
    high_throughput_enabled: true,
    fallback_legacy_rate: LegacyRate::Ofdm54M,
    fallback_ht_mcs: HtMcs::Mcs7,
    fallback_ht_guard_interval: HtGuardInterval::Long800Ns,
    ht_mcs_override: None,
    ht_guard_interval_override: None,
    he_mcs_override: None,
    he_guard_interval_and_ltf_override: None,
    he_dcm_override: None,
    he_800ns_gi_ltf: HeGuardIntervalAndLtf::TwoLtf800Ns,
    peer_supports_ht_short_guard_interval: false,
    peer_supports_ldpc: true,
    peer_dcm_receive: HeDcmConstellation::Bpsk,
};

#[test]
fn sta_tx_policy_joins_he_schedule_peer_ltf_and_ldpc() {
    let rate =
        HE20_POLICY.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap());
    assert_eq!(
        rate,
        TxPhyRate::He(HeRate::ldpc(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
        ))
    );
}

#[test]
fn sta_tx_policy_keeps_hil_override_and_unknown_arena_explicit() {
    let ht40 = StaTxRatePolicy {
        association_phy: PhyMode::Ht40,
        ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
        peer_supports_ht_short_guard_interval: true,
        peer_supports_ldpc: false,
        ..HE20_POLICY
    };
    assert_eq!(
        ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    let fixed_mcs = StaTxRatePolicy {
        ht_mcs_override: Some(HtMcs::Mcs3),
        ..ht40
    };
    assert_eq!(
        fixed_mcs.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs3,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Lora, 0).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        StaTxRatePolicy {
            high_throughput_enabled: false,
            ..HE20_POLICY
        }
        .rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap()),
        TxPhyRate::Legacy(LegacyRate::Ofdm54M)
    );
}

#[test]
fn sta_tx_policy_fixed_ht_matrix_preserves_negotiated_width() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap();
    for association_phy in [PhyMode::Ht20, PhyMode::Ht40] {
        let width = match association_phy {
            PhyMode::Ht20 => HtChannelWidth::Mhz20,
            PhyMode::Ht40 => HtChannelWidth::Mhz40,
            _ => unreachable!(),
        };
        for mcs_index in 0..=7 {
            let mcs = HtMcs::from_index(mcs_index).unwrap();
            for guard_interval in [HtGuardInterval::Long800Ns, HtGuardInterval::Short400Ns] {
                let policy = StaTxRatePolicy {
                    association_phy,
                    ht_mcs_override: Some(mcs),
                    ht_guard_interval_override: Some(guard_interval),
                    peer_supports_ht_short_guard_interval: true,
                    peer_supports_ldpc: false,
                    ..HE20_POLICY
                };
                assert_eq!(
                    policy.rate_for_schedule(schedule),
                    TxPhyRate::Ht(HtRate::new(mcs, guard_interval, width))
                );
            }
        }
    }
}

#[test]
fn sta_tx_policy_never_publishes_unadvertised_ht_short_gi() {
    let policy = StaTxRatePolicy {
        association_phy: PhyMode::Ht40,
        fallback_ht_guard_interval: HtGuardInterval::Short400Ns,
        ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
        peer_supports_ht_short_guard_interval: false,
        ..HE20_POLICY
    };
    assert_eq!(
        policy.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        policy.fallback_rate(),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))
    );
}

#[test]
fn sta_tx_policy_fixed_he_su_matrix_preserves_peer_coding() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
    let gi_ltf_values = [
        HeGuardIntervalAndLtf::OneLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];
    for mcs_index in 0..=9 {
        let mcs = HeMcs::from_index(mcs_index).unwrap();
        for guard_interval_and_ltf in gi_ltf_values {
            let policy = StaTxRatePolicy {
                he_mcs_override: Some(mcs),
                he_guard_interval_and_ltf_override: Some(guard_interval_and_ltf),
                ..HE20_POLICY
            };
            assert_eq!(
                policy.rate_for_schedule(schedule),
                TxPhyRate::He(HeRate::ldpc(mcs, guard_interval_and_ltf))
            );
        }
    }
}

#[test]
fn sta_tx_policy_dcm_override_is_capability_gated_and_preserves_coding() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
    let gi = HeGuardIntervalAndLtf::TwoLtf800Ns;
    let bpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs0, gi);
    let bpsk_policy = StaTxRatePolicy {
        he_dcm_override: Some(bpsk),
        peer_dcm_receive: HeDcmConstellation::Bpsk,
        ..HE20_POLICY
    };
    assert!(bpsk_policy.he_dcm_override_is_supported());
    assert_eq!(
        bpsk_policy.rate_for_schedule(schedule),
        TxPhyRate::He(bpsk.rate())
    );
    assert!(!bpsk.rate().is_ldpc());

    let qpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs1, gi);
    let unsupported_qpsk = StaTxRatePolicy {
        he_dcm_override: Some(qpsk),
        peer_dcm_receive: HeDcmConstellation::Bpsk,
        ..HE20_POLICY
    };
    assert!(!unsupported_qpsk.he_dcm_override_is_supported());
    let TxPhyRate::He(fallback) = unsupported_qpsk.rate_for_schedule(schedule) else {
        panic!("HE association retains the ordinary HE schedule");
    };
    assert!(!fallback.is_dcm());

    let supported_qpsk = StaTxRatePolicy {
        peer_dcm_receive: HeDcmConstellation::Qpsk,
        ..unsupported_qpsk
    };
    assert_eq!(
        supported_qpsk.rate_for_schedule(schedule),
        TxPhyRate::He(qpsk.rate())
    );

    let ldpc_16qam = HeDcmRate::ldpc(crate::tx::HeLdpcDcmMcs::Mcs4, gi);
    let no_ldpc = StaTxRatePolicy {
        he_dcm_override: Some(ldpc_16qam),
        peer_dcm_receive: HeDcmConstellation::Qam16,
        peer_supports_ldpc: false,
        ..HE20_POLICY
    };
    assert!(!no_ldpc.he_dcm_override_is_supported());
    let with_ldpc = StaTxRatePolicy {
        peer_supports_ldpc: true,
        ..no_ldpc
    };
    assert!(with_ldpc.he_dcm_override_is_supported());
    assert_eq!(
        with_ldpc.rate_for_schedule(schedule),
        TxPhyRate::He(ldpc_16qam.rate())
    );
}

#[test]
fn association_owns_ordinary_ampdu_and_completion_rate_transitions() {
    let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.tx_rate(HE20_POLICY),
        HE20_POLICY.rate_for_schedule(association.current_schedule())
    );
    assert_eq!(
        association.ampdu_tx_rate(HE20_POLICY),
        HE20_POLICY.rate_for_schedule(association.current_ampdu_schedule().unwrap())
    );
    association.observe_tx_completion(
        model_completion(crate::tx::TxCookie(1), 0, 0).with_ack_snr_encoded_model(0xeb),
    );
    assert_eq!(association.latest_ack_snr(), Some(75));
}

#[test]
fn basic_ofdm_control_schedule_walks_six_then_one_megabit_within_thirty_two_publications() {
    use crate::{
        rate::schedule::schedule_publication_limit,
        tx::{TxPhyRate, runtime::select_schedule_retry_rate},
    };
    let schedule = association(StaRateControlPhy::He, false).control_schedule();
    for attempt in 0..7 {
        assert_eq!(
            select_schedule_retry_rate(schedule, attempt),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
        );
    }
    assert_eq!(
        select_schedule_retry_rate(schedule, 7),
        Ok(TxPhyRate::Legacy(LegacyRate::Dsss1MLong))
    );
    assert_eq!(schedule_publication_limit(schedule), 32);
    assert_eq!(schedule_publication_limit(DEFAULT_CONTROL_SCHEDULE), 32);
    assert_eq!(
        select_schedule_retry_rate(DEFAULT_CONTROL_SCHEDULE, 0),
        Ok(TxPhyRate::Legacy(LegacyRate::Dsss1MLong))
    );
}
