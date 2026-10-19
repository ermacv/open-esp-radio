use super::*;
use crate::{
    rx::HeGuardIntervalAndLtf,
    tx::{HeMcs, HeRate, HtGuardInterval, HtMcs, HtRate},
};

/// Basic {1, 2, 5.5, 11} with all ERP-OFDM rates supported but not basic,
/// the hostapd 802.11g default.
const DSSS_BASIC: ([u8; 8], [u8; 4]) = (
    [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
    [0x30, 0x48, 0x60, 0x6c],
);
/// Basic {6, 12, 24} OFDM rates only.
const OFDM_BASIC: ([u8; 8], [u8; 4]) = (
    [0x02, 0x04, 0x0b, 0x16, 0x8c, 0x12, 0x98, 0x24],
    [0xb0, 0x48, 0x60, 0x6c],
);

fn basic(rates: ([u8; 8], [u8; 4])) -> BasicRates {
    BasicRates::from_rate_elements(&rates.0, &rates.1)
}

fn policy(bss: BssProtection) -> WifiTxProtectionPolicy {
    let mut policy = WifiTxProtectionPolicy::default();
    policy.install_bss(bss);
    policy
}

fn bss(erp: ErpProtection, ht: HtProtectionMode, basic_rates: BasicRates) -> BssProtection {
    BssProtection {
        erp,
        ht,
        he_txop_rts_threshold: None,
        he_packet_padding: HePacketPadding::None,
        basic_rates,
        short_preamble: true,
    }
}

const fn ht(mcs: HtMcs, width: HtChannelWidth) -> TxPhyRate {
    TxPhyRate::Ht(HtRate::new(mcs, HtGuardInterval::Long800Ns, width))
}

const fn he(mcs: HeMcs) -> HeRate {
    HeRate::new(mcs, HeGuardIntervalAndLtf::TwoLtf800Ns)
}

fn ppdu(rate: TxPhyRate, receiver: TxReceiver, length: u32) -> ProtectedPpdu {
    ProtectedPpdu {
        rate,
        receiver,
        psdu: TxPsdu::Mpdu { length },
    }
}

fn ampdu(rate: TxPhyRate, receiver: TxReceiver, length: u32) -> ProtectedPpdu {
    ProtectedPpdu {
        rate,
        receiver,
        psdu: TxPsdu::Ampdu { length },
    }
}

#[test]
fn receiver_is_classified_by_address_one_only() {
    assert_eq!(
        TxReceiver::from_address1(&[0x02, 0, 0, 0, 0, 1]),
        TxReceiver::Individual
    );
    assert_eq!(TxReceiver::from_address1(&[0xff; 6]), TxReceiver::Group);
    assert_eq!(
        TxReceiver::from_address1(&[0x01, 0, 0x5e, 0, 0, 1]),
        TxReceiver::Group
    );
}

#[test]
fn erp_protects_non_dsss_ppdus_with_cts_to_self_at_a_dsss_rate() {
    let policy = policy(bss(
        ErpProtection::from_information(Some(0x03)),
        HtProtectionMode::None,
        basic(DSSS_BASIC),
    ));
    for receiver in [TxReceiver::Individual, TxReceiver::Group] {
        let decision = policy.select(ppdu(TxPhyRate::Legacy(LegacyRate::Ofdm24M), receiver, 100));
        assert_eq!(
            decision.protection,
            TxProtection::CtsToSelf {
                rate: LegacyRate::Cck11MShort
            }
        );
        assert_eq!(decision.reasons, TxProtectionReasons::ERP);
    }
    let decision = policy.select(ppdu(
        ht(HtMcs::Mcs7, HtChannelWidth::Mhz20),
        TxReceiver::Individual,
        100,
    ));
    assert_eq!(
        decision.protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck11MShort
        }
    );
    assert_eq!(
        policy
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Cck11MShort),
                TxReceiver::Individual,
                100,
            ))
            .protection,
        TxProtection::None
    );
}

#[test]
fn erp_control_frames_obey_barker_preamble_mode_and_bss_short_preamble() {
    let barker = policy(bss(
        ErpProtection::from_information(Some(0x06)),
        HtProtectionMode::None,
        basic(DSSS_BASIC),
    ));
    assert_eq!(
        barker
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                TxReceiver::Group,
                100
            ))
            .protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck11MLong
        }
    );
    let mut long_only = bss(
        ErpProtection::new(true, false),
        HtProtectionMode::None,
        basic(DSSS_BASIC),
    );
    long_only.short_preamble = false;
    assert_eq!(
        policy(long_only)
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Ofdm6M),
                TxReceiver::Group,
                100
            ))
            .protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck5M5Long
        }
    );
}

#[test]
fn erp_without_dsss_basic_rates_uses_the_erp_mandatory_dsss_rates() {
    let policy = policy(bss(
        ErpProtection::new(true, false),
        HtProtectionMode::None,
        basic(OFDM_BASIC),
    ));
    assert_eq!(
        policy
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Ofdm9M),
                TxReceiver::Group,
                100
            ))
            .protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck5M5Short
        }
    );
}

#[test]
fn ht_protection_uses_rts_for_individual_and_cts_to_self_for_group_receivers() {
    let policy = policy(bss(
        ErpProtection::NONE,
        HtProtectionMode::Nonmember,
        basic(OFDM_BASIC),
    ));
    let rate = ht(HtMcs::Mcs7, HtChannelWidth::Mhz40);
    let unicast = policy.select(ppdu(rate, TxReceiver::Individual, 100));
    assert_eq!(
        unicast.protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm24M
        }
    );
    assert_eq!(unicast.reasons, TxProtectionReasons::HT);
    assert_eq!(
        policy.select(ppdu(rate, TxReceiver::Group, 100)).protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Ofdm24M
        }
    );
    assert_eq!(
        policy
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                TxReceiver::Individual,
                100
            ))
            .protection,
        TxProtection::None
    );
}

#[test]
fn twenty_mhz_mode_protects_only_forty_mhz_ppdus() {
    let policy = policy(bss(
        ErpProtection::NONE,
        HtProtectionMode::TwentyMhz,
        basic(OFDM_BASIC),
    ));
    assert_eq!(
        policy
            .select(ppdu(
                ht(HtMcs::Mcs7, HtChannelWidth::Mhz20),
                TxReceiver::Individual,
                100
            ))
            .protection,
        TxProtection::None
    );
    assert!(matches!(
        policy
            .select(ppdu(
                ht(HtMcs::Mcs7, HtChannelWidth::Mhz40),
                TxReceiver::Individual,
                100
            ))
            .protection,
        TxProtection::RtsCts { .. }
    ));
    assert_eq!(
        policy
            .select(ppdu(
                TxPhyRate::He(he(HeMcs::Mcs7)),
                TxReceiver::Individual,
                100
            ))
            .protection,
        TxProtection::None
    );
}

#[test]
fn control_rate_is_the_fastest_basic_rate_not_faster_than_the_data_rate() {
    let policy = policy(bss(
        ErpProtection::NONE,
        HtProtectionMode::NonHtMixed,
        basic(OFDM_BASIC),
    ));
    let control = |mcs| {
        policy
            .select(ppdu(
                ht(mcs, HtChannelWidth::Mhz20),
                TxReceiver::Individual,
                100,
            ))
            .protection
            .control_rate()
    };
    assert_eq!(control(HtMcs::Mcs0), Some(LegacyRate::Ofdm6M));
    assert_eq!(control(HtMcs::Mcs1), Some(LegacyRate::Ofdm12M));
    assert_eq!(control(HtMcs::Mcs3), Some(LegacyRate::Ofdm24M));

    let dsss = policy_with_basic(basic(DSSS_BASIC));
    assert_eq!(
        dsss.select(ppdu(
            ht(HtMcs::Mcs7, HtChannelWidth::Mhz20),
            TxReceiver::Individual,
            100
        ))
        .protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Cck11MShort
        }
    );
}

fn policy_with_basic(basic_rates: BasicRates) -> WifiTxProtectionPolicy {
    policy(bss(
        ErpProtection::NONE,
        HtProtectionMode::NonHtMixed,
        basic_rates,
    ))
}

#[test]
fn a_slower_data_rate_than_every_basic_rate_uses_the_slowest_basic_rate() {
    let policy = policy(bss(
        ErpProtection::NONE,
        HtProtectionMode::None,
        basic(OFDM_BASIC),
    ));
    assert_eq!(
        policy
            .select(ppdu(
                TxPhyRate::Legacy(LegacyRate::Dsss2MLong),
                TxReceiver::Individual,
                3000
            ))
            .protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm6M
        }
    );
}

#[test]
fn length_threshold_requests_rts_only_above_the_threshold_and_never_for_groups() {
    let policy = WifiTxProtectionPolicy::default();
    let rate = TxPhyRate::Legacy(LegacyRate::Ofdm54M);
    assert_eq!(
        policy
            .select(ppdu(rate, TxReceiver::Individual, 2346))
            .protection,
        TxProtection::None
    );
    let long = policy.select(ppdu(rate, TxReceiver::Individual, 2347));
    assert_eq!(
        long.protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm24M
        }
    );
    assert_eq!(long.reasons, TxProtectionReasons::LENGTH);
    assert_eq!(
        policy
            .select(ppdu(rate, TxReceiver::Group, 4000))
            .protection,
        TxProtection::None
    );

    let mut disabled = policy;
    disabled.set_rts_length_threshold(None);
    assert_eq!(
        disabled
            .select(ppdu(rate, TxReceiver::Individual, 65_000))
            .protection,
        TxProtection::None
    );
}

#[test]
fn an_aggregate_is_protected_only_by_bss_rules_never_by_its_length() {
    let policy = WifiTxProtectionPolicy::default();
    let rate = ht(HtMcs::Mcs7, HtChannelWidth::Mhz40);
    // A full BA32 aggregate is far above dot11RTSThreshold, but the vendor
    // A-MPDU path never consults the single-MPDU length rule.
    let aggregate = policy.select(ampdu(rate, TxReceiver::Individual, 48_000));
    assert_eq!(aggregate.protection, TxProtection::None);
    assert!(aggregate.reasons.is_empty());
    assert_eq!(
        policy
            .select(ppdu(rate, TxReceiver::Individual, 2_347))
            .reasons,
        TxProtectionReasons::LENGTH
    );
    let protected = self::policy(bss(
        ErpProtection::new(false, false),
        HtProtectionMode::NonHtMixed,
        basic(OFDM_BASIC),
    ));
    assert_eq!(
        protected
            .select(ampdu(rate, TxReceiver::Individual, 48_000))
            .reasons,
        TxProtectionReasons::HT
    );
}

#[test]
fn rts_supersedes_cts_to_self_and_keeps_the_erp_dsss_control_rate() {
    let policy = policy(bss(
        ErpProtection::new(true, false),
        HtProtectionMode::Nonmember,
        basic(DSSS_BASIC),
    ));
    let decision = policy.select(ppdu(
        ht(HtMcs::Mcs7, HtChannelWidth::Mhz20),
        TxReceiver::Individual,
        4000,
    ));
    assert_eq!(
        decision.protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Cck11MShort
        }
    );
    assert!(decision.reasons.contains(TxProtectionReasons::ERP));
    assert!(decision.reasons.contains(TxProtectionReasons::HT));
    assert!(decision.reasons.contains(TxProtectionReasons::LENGTH));
}

#[test]
fn he_txop_threshold_matches_the_vendor_byte_budget() {
    let rule = HeTxopRtsRule::new(
        HeTxopDurationRtsThreshold::new(64).unwrap(),
        HePacketPadding::None,
    );
    // 64 * 32 - 20 - 23.2 - 16 = 1988.8 us; minus the 32-us BlockAck
    // estimate is 143 whole 13.6-us symbols of 1170 bits at MCS7.
    assert_eq!(rule.maximum_unprotected_apep_bytes(he(HeMcs::Mcs7)), 20_911);
    let padded = HeTxopRtsRule::new(rule.threshold(), HePacketPadding::Us16);
    assert!(
        padded.maximum_unprotected_apep_bytes(he(HeMcs::Mcs7))
            < rule.maximum_unprotected_apep_bytes(he(HeMcs::Mcs7))
    );
    let short = HeTxopRtsRule::new(
        HeTxopDurationRtsThreshold::new(2).unwrap(),
        HePacketPadding::None,
    );
    assert_eq!(short.maximum_unprotected_apep_bytes(he(HeMcs::Mcs0)), 0);
}

#[test]
fn he_txop_threshold_requests_rts_for_individual_he_ppdus_above_the_budget() {
    let rule = HeTxopRtsRule::new(
        HeTxopDurationRtsThreshold::new(64).unwrap(),
        HePacketPadding::None,
    );
    let mut bss = bss(
        ErpProtection::NONE,
        HtProtectionMode::None,
        basic(OFDM_BASIC),
    );
    bss.he_txop_rts_threshold = Some(rule.threshold());
    let mut policy = policy(bss);
    policy.set_rts_length_threshold(None);
    let rate = he(HeMcs::Mcs7);
    let budget = u32::from(rule.maximum_unprotected_apep_bytes(rate));
    assert_eq!(
        policy
            .select(ampdu(TxPhyRate::He(rate), TxReceiver::Individual, budget))
            .protection,
        TxProtection::None
    );
    let decision = policy.select(ppdu(
        TxPhyRate::He(rate),
        TxReceiver::Individual,
        budget + 1,
    ));
    assert_eq!(
        decision.protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm24M
        }
    );
    assert_eq!(decision.reasons, TxProtectionReasons::HE_TXOP_DURATION);
    assert_eq!(
        policy
            .select(ampdu(TxPhyRate::He(rate), TxReceiver::Group, budget + 1))
            .protection,
        TxProtection::None
    );
}

#[test]
fn leaving_the_bss_keeps_only_the_local_length_threshold() {
    let mut policy = policy(bss(
        ErpProtection::new(true, true),
        HtProtectionMode::NonHtMixed,
        basic(DSSS_BASIC),
    ));
    policy.clear_bss();
    assert_eq!(policy.bss(), BssProtection::UNPROTECTED);
    assert_eq!(
        policy.rts_length_threshold(),
        Some(RtsLengthThreshold::VENDOR_DEFAULT)
    );
}
