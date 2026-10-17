use oer_ieee80211_mac::phy::{FecCoding, HeGiLtf, HeMcs, HtMcs, HtRate, SpatialStreams};

use super::*;

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

/// A budget of a fixed byte count, whatever the rate.
struct Budget(u16);

impl HeTxopRtsBudget for Budget {
    fn max_unprotected_apep_bytes(
        &self,
        _threshold: HeTxopDurationRtsThreshold,
        _padding: HePacketPadding,
        _rate: HeRate,
    ) -> u16 {
        self.0
    }
}

fn basic(rates: ([u8; 8], [u8; 4])) -> BasicRates {
    BasicRates::from_rate_elements(&rates.0, &rates.1)
}

fn policy(erp: ErpProtection, ht: HtProtectionMode, basic_rates: BasicRates) -> ProtectionPolicy {
    let mut policy = ProtectionPolicy::new(Some(RtsLengthThreshold::new(2346)));
    policy.install_bss(BssProtection {
        erp,
        ht,
        basic_rates,
        short_preamble: true,
        ..BssProtection::UNPROTECTED
    });
    policy
}

fn ht(mcs: u8, bandwidth: PpduBandwidth) -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), bandwidth, false).unwrap())
}

fn he(mcs: u8) -> HeRate {
    HeRate::new(
        HeMcs::new(mcs).unwrap(),
        SpatialStreams::ONE,
        PpduBandwidth::Mhz20,
        HeGiLtf::Ltf2xGi800Ns,
        FecCoding::Bcc,
        false,
    )
    .unwrap()
}

fn mpdu(rate: PhyRate, receiver: TxReceiver, length: u32) -> ProtectedPpdu {
    ProtectedPpdu {
        rate,
        receiver,
        psdu: TxPsdu::Mpdu { length },
    }
}

fn ampdu(rate: PhyRate, receiver: TxReceiver, length: u32) -> ProtectedPpdu {
    ProtectedPpdu {
        rate,
        receiver,
        psdu: TxPsdu::Ampdu { length },
    }
}

const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);
const CCK11_SHORT: LegacyRate = LegacyRate::Cck11M(DsssPreamble::Short);

#[test]
fn erp_protection_sends_cts_to_self_at_a_dsss_rate_for_every_receiver() {
    let policy = policy(
        ErpProtection::from_information(Some(0x03)),
        HtProtectionMode::None,
        basic(DSSS_BASIC),
    );
    for receiver in [TxReceiver::Individual, TxReceiver::Group] {
        let decision = policy.select(mpdu(OFDM24, receiver, 100), &ProtectEveryHeTxop);
        assert_eq!(
            decision.protection,
            TxProtection::CtsToSelf { rate: CCK11_SHORT }
        );
        assert_eq!(decision.reasons, TxProtectionReasons::ERP);
        assert_eq!(decision.protection.port_protection(), Protection::CtsToSelf);
    }
    let dsss = PhyRate::Legacy(CCK11_SHORT);
    assert_eq!(
        policy
            .select(mpdu(dsss, TxReceiver::Individual, 100), &ProtectEveryHeTxop)
            .protection,
        TxProtection::None
    );
}

#[test]
fn barker_preamble_mode_keeps_dsss_control_frames_long() {
    let barker = policy(
        ErpProtection::from_information(Some(0x06)),
        HtProtectionMode::None,
        basic(DSSS_BASIC),
    );
    assert_eq!(
        barker
            .select(
                mpdu(PhyRate::Legacy(LegacyRate::Ofdm54M), TxReceiver::Group, 100),
                &ProtectEveryHeTxop
            )
            .protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck11M(DsssPreamble::Long)
        }
    );
}

#[test]
fn ht_protection_uses_rts_for_individual_and_cts_to_self_for_group_receivers() {
    let policy = policy(
        ErpProtection::NONE,
        HtProtectionMode::Nonmember,
        basic(OFDM_BASIC),
    );
    let rate = ht(7, PpduBandwidth::Mhz40);
    let unicast = policy.select(mpdu(rate, TxReceiver::Individual, 100), &ProtectEveryHeTxop);
    assert_eq!(
        unicast.protection,
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm24M
        }
    );
    assert_eq!(unicast.protection.port_protection(), Protection::RtsCts);
    assert_eq!(
        policy
            .select(mpdu(rate, TxReceiver::Group, 100), &ProtectEveryHeTxop)
            .protection,
        TxProtection::CtsToSelf {
            rate: LegacyRate::Ofdm24M
        }
    );
    // Twenty-megahertz mode protects only forty-megahertz PPDUs.
    let twenty = self::policy(
        ErpProtection::NONE,
        HtProtectionMode::TwentyMhz,
        basic(OFDM_BASIC),
    );
    assert_eq!(
        twenty
            .select(
                mpdu(ht(7, PpduBandwidth::Mhz20), TxReceiver::Individual, 100),
                &ProtectEveryHeTxop
            )
            .protection,
        TxProtection::None
    );
}

#[test]
fn control_rate_is_the_fastest_basic_rate_not_faster_than_the_data_rate() {
    let policy = policy(
        ErpProtection::NONE,
        HtProtectionMode::NonHtMixed,
        basic(OFDM_BASIC),
    );
    assert_eq!(
        policy.control_rate(ht(0, PpduBandwidth::Mhz20)),
        LegacyRate::Ofdm6M
    );
    assert_eq!(
        policy.control_rate(ht(1, PpduBandwidth::Mhz20)),
        LegacyRate::Ofdm12M
    );
    assert_eq!(
        policy.control_rate(ht(3, PpduBandwidth::Mhz20)),
        LegacyRate::Ofdm24M
    );
    // Slower than every basic rate: the slowest.
    assert_eq!(
        policy.control_rate(PhyRate::Legacy(LegacyRate::Dsss2M(DsssPreamble::Long))),
        LegacyRate::Ofdm6M
    );
    // ERP without a DSSS basic rate: the ERP mandatory DSSS rates.
    let erp = self::policy(
        ErpProtection::new(true, false),
        HtProtectionMode::None,
        basic(OFDM_BASIC),
    );
    assert_eq!(
        erp.control_rate(PhyRate::Legacy(LegacyRate::Ofdm9M)),
        LegacyRate::Cck5M5(DsssPreamble::Short)
    );
}

#[test]
fn every_basic_set_selects_the_required_control_rate() {
    for mask in 0..1 << BasicRates::ORDER.len() {
        for dsss_only in [false, true] {
            let selected = BasicRates(mask);
            let eligible = |rate: &LegacyRate| !dsss_only || !rate.is_ofdm();
            let candidates = if selected.rates().any(|rate| eligible(&rate)) {
                selected
            } else {
                BasicRates::ERP_MANDATORY
            };
            let policy = policy(
                ErpProtection::new(dsss_only, false),
                HtProtectionMode::None,
                selected,
            );
            for (_, data) in BasicRates::ORDER {
                let expected = candidates
                    .rates()
                    .filter(eligible)
                    .map(LegacyRate::kbps)
                    .filter(|kbps| *kbps <= data.kbps())
                    .max()
                    .unwrap_or_else(|| {
                        candidates
                            .rates()
                            .filter(eligible)
                            .map(LegacyRate::kbps)
                            .min()
                            .unwrap()
                    });
                let actual = policy.control_rate(PhyRate::Legacy(data));
                assert_eq!(
                    actual.kbps(),
                    expected,
                    "set {mask:#x}, ERP {dsss_only}, data {data:?}"
                );
                assert!(!dsss_only || !actual.is_ofdm());
            }
        }
    }
}

#[test]
fn the_length_threshold_protects_only_long_individual_mpdus() {
    let policy = ProtectionPolicy::new(Some(RtsLengthThreshold::new(2346)));
    let rate = PhyRate::Legacy(LegacyRate::Ofdm54M);
    let select = |ppdu| policy.select(ppdu, &ProtectEveryHeTxop);
    assert_eq!(
        select(mpdu(rate, TxReceiver::Individual, 2346)).protection,
        TxProtection::None
    );
    let long = select(mpdu(rate, TxReceiver::Individual, 2347));
    assert_eq!(long.reasons, TxProtectionReasons::LENGTH);
    assert!(matches!(long.protection, TxProtection::RtsCts { .. }));
    assert_eq!(
        select(mpdu(rate, TxReceiver::Group, 4000)).protection,
        TxProtection::None
    );
    // An aggregate is protected only by the BSS rules.
    assert_eq!(
        select(ampdu(
            ht(7, PpduBandwidth::Mhz40),
            TxReceiver::Individual,
            48_000
        ))
        .protection,
        TxProtection::None
    );
}

#[test]
fn the_he_txop_budget_protects_individual_he_ppdus_above_it() {
    let mut policy = policy(
        ErpProtection::NONE,
        HtProtectionMode::None,
        basic(OFDM_BASIC),
    );
    let mut bss = policy.bss();
    bss.he_txop_rts_threshold = HeTxopDurationRtsThreshold::new(64);
    policy.install_bss(bss);
    policy.set_rts_length_threshold(None);
    let rate = PhyRate::He(he(7));
    let budget = Budget(1_000);
    assert_eq!(
        policy
            .select(ampdu(rate, TxReceiver::Individual, 1_000), &budget)
            .protection,
        TxProtection::None
    );
    let decision = policy.select(ampdu(rate, TxReceiver::Individual, 1_001), &budget);
    assert_eq!(decision.reasons, TxProtectionReasons::HE_TXOP_DURATION);
    assert!(matches!(decision.protection, TxProtection::RtsCts { .. }));
    assert_eq!(
        policy
            .select(ampdu(rate, TxReceiver::Group, 1_001), &budget)
            .protection,
        TxProtection::None
    );
    // Without an estimate every individual HE PPDU is protected.
    assert!(matches!(
        policy
            .select(mpdu(rate, TxReceiver::Individual, 1), &ProtectEveryHeTxop)
            .protection,
        TxProtection::RtsCts { .. }
    ));
    assert!(HeTxopDurationRtsThreshold::new(0).is_none());
    assert!(HeTxopDurationRtsThreshold::new(1023).is_none());
}

#[test]
fn leaving_the_bss_keeps_only_the_local_length_threshold() {
    let mut policy = policy(
        ErpProtection::new(true, true),
        HtProtectionMode::NonHtMixed,
        basic(DSSS_BASIC),
    );
    policy.clear_bss();
    assert_eq!(policy.bss(), BssProtection::UNPROTECTED);
    assert_eq!(
        policy.rts_length_threshold(),
        Some(RtsLengthThreshold::new(2346))
    );
    assert_eq!(TxReceiver::from_address1(&[0xff; 6]), TxReceiver::Group);
}
