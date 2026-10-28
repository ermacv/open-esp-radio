use oer_esp32s31_ieee80211_mac::{
    edca::*,
    tx::{runtime::*, *},
};
use oer_espressif_ieee80211_policy::{backend::EspressifAmpduRetryState, lmac};
use oer_ieee80211_port::backend::*;
use oer_ieee80211_port::backend_aggregate::*;
use oer_ieee80211_wire::{
    qos::{WmmAccessCategory, WmmUserPriority},
    sequence::SequenceNumber,
};
type AmpduRetryState<const C: usize> = EspressifAmpduRetryState<C>;
const VENDOR_SHORT_RETRY_LIMIT: u8 = lmac::SHORT_RETRY_LIMIT;
const VENDOR_AMPDU_MSDU_LIFETIME_MICROS: u32 = lmac::AMPDU_MSDU_LIFETIME_MICROS;

use oer_esp32s31_ieee80211_mac::tx::test_support::model_completion;
use oer_esp32s31_ieee80211_mac::tx::{
    LegacyRate, TxCompletion, TxCookie,
    ampdu::{HtAmpduTxCompletion, HtBlockAckObservation},
};
use oer_ieee80211_port::VifRole;
use oer_ieee80211_wire::extensions::wmm::parse_wmm_parameter_element;

const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

fn ipv4_with_dscp(dscp: u8) -> [u8; 16] {
    let mut frame = [0_u8; 16];
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[14] = 0x45;
    frame[15] = dscp << 2;
    frame
}

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
            oer_ieee80211_port::EdcaParameterSet::from_wmm(
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
fn negotiated_acm_downgrades_tid_and_queue_without_claiming_admission() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_edca(
            VifRole::Station,
            oer_ieee80211_port::EdcaParameterSet::from_wmm(
                parse_wmm_parameter_element(&STANDARD_WMM).unwrap(),
            ),
        )
        .unwrap();

    let voice = policy
        .select_network_traffic(VifRole::Station, &ipv4_with_dscp(46), true)
        .unwrap();
    assert_eq!(voice.requested.access_category, WmmAccessCategory::Voice);
    assert_eq!(voice.access_category, WmmAccessCategory::Video);
    assert_eq!(voice.user_priority, WmmUserPriority::UP5);
    assert_eq!(voice.queue(), LegacyTxQueue::Video);
    assert_eq!(
        voice.admission,
        WmmAdmissionDisposition::Downgraded {
            requested: WmmAccessCategory::Voice
        }
    );
    assert_eq!(voice.txop_limit_units_32_us(), 94);

    let non_qos = policy
        .select_network_traffic(VifRole::Station, &ipv4_with_dscp(46), false)
        .unwrap();
    assert_eq!(non_qos.tid(), 0);
    assert_eq!(non_qos.queue(), LegacyTxQueue::BestEffort);
    assert_eq!(non_qos.admission, WmmAdmissionDisposition::NonQosBestEffort);
}

#[test]
fn all_mandatory_access_categories_fail_closed() {
    let mut element = STANDARD_WMM;
    for offset in [10, 14, 18, 22] {
        element[offset] |= 0x10;
    }
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_edca(
            VifRole::Station,
            oer_ieee80211_port::EdcaParameterSet::from_wmm(
                parse_wmm_parameter_element(&element).unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        policy.select_network_traffic(VifRole::Station, &ipv4_with_dscp(0), true),
        Err(WifiTxTrafficError::AdmissionControlRequired {
            requested: WmmAccessCategory::BestEffort
        })
    );
}

#[test]
fn negotiated_txop_is_bounded_and_medium_ownership_stays_unsupported() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_edca(
            VifRole::Station,
            oer_ieee80211_port::EdcaParameterSet::from_wmm(
                parse_wmm_parameter_element(&STANDARD_WMM).unwrap(),
            ),
        )
        .unwrap();
    let video = policy
        .select_network_traffic(VifRole::Station, &ipv4_with_dscp(40), true)
        .unwrap();
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
        Ok(HeEdcaTxopLimit::from_units_32_us(94).unwrap())
    );
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
        Ok(HeEdcaTxopLimit::from_units_32_us(47).unwrap())
    );
    assert_eq!(
        video.require_ht_txop_support(),
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

    let mut wide = STANDARD_WMM;
    wide[21] = 1;
    policy
        .install_edca(
            VifRole::Station,
            oer_ieee80211_port::EdcaParameterSet::from_wmm(
                parse_wmm_parameter_element(&wide).unwrap(),
            ),
        )
        .unwrap();
    let video = policy
        .select_network_traffic(VifRole::Station, &ipv4_with_dscp(40), true)
        .unwrap();
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
        Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us: 350 })
    );
}

#[test]
fn ordinary_he_retries_walk_the_dot11ax_record() {
    use oer_esp32s31_ieee80211_mac::rx::HeGuardIntervalAndLtf;
    use oer_esp32s31_ieee80211_mac::tx::{HeMcs, HeRate};
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
        oer_ieee80211_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(0),
            &oer_espressif_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(mcs9)
    );
    assert_eq!(
        oer_ieee80211_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(2),
            &oer_espressif_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs7,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );
    assert_eq!(
        oer_ieee80211_upper_mac::retry::select_ordinary_retry_rate(
            mcs9.phy_rate(),
            counters(4),
            &oer_espressif_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    // An 800-ns rate below MCS9 has no record and keeps its rate.
    let mcs8 = TxPhyRate::He(HeRate::new(HeMcs::Mcs8, HeGuardIntervalAndLtf::OneLtf800Ns));
    assert_eq!(
        oer_ieee80211_upper_mac::retry::select_ordinary_retry_rate(
            mcs8.phy_rate(),
            counters(4),
            &oer_espressif_ieee80211_policy::retry_ladder::EspressifRetryLadder
        )
        .and_then(|rate| TxPhyRate::try_from(rate).ok()),
        Some(mcs8)
    );
}

fn completion_with_tx(
    tx: TxCompletion,
    starting_sequence: u16,
    bitmap: u64,
) -> HtAmpduTxCompletion {
    HtAmpduTxCompletion {
        tx,
        block_ack: HtBlockAckObservation::new(
            0,
            SequenceNumber::new(starting_sequence).unwrap(),
            bitmap,
        ),
        block_ack_received: true,
    }
}

fn completion(status: u8, starting_sequence: u16, bitmap: u64) -> HtAmpduTxCompletion {
    completion_with_tx(
        model_completion(TxCookie(1), status, 0),
        starting_sequence,
        bitmap,
    )
}

const HT_POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
    lifetime: oer_time::Duration::from_micros(VENDOR_AMPDU_MSDU_LIFETIME_MICROS as u64),
    retain_single_mpdu: false,
};

#[test]
fn partial_block_ack_compacts_sequences_across_retained_attempts() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(0x0ffe).unwrap(), 4, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(
            completion(0, 0x0ffe, 0b0101),
            4,
            mono(0),
            true,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 })
    );
    assert_eq!(state.current_subframes(), 2);
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.block_ack_mpdu_attempts(), 4);

    // The retained sequences are 0x0fff and 1. A BlockAck starting at
    // 0x0fff acknowledges both across the 12-bit wrap.
    assert_eq!(
        state.observe(
            completion(0, 0x0fff, 0b0101),
            2,
            mono(0),
            true,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 4);
    assert_eq!(state.aggregate_attempts(), 2);
    assert_eq!(state.block_ack_mpdu_attempts(), 6);
}

#[test]
fn cts_timeout_republishes_the_unchanged_aggregate_then_requests_its_block_ack() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(40).unwrap(), 3, HT_POLICY).unwrap();
    // Status two is a CTS timeout; a stale bitmap must not be consumed.
    let cts_timeout = completion(2, 40, u64::MAX);
    for failure in 1..VENDOR_SHORT_RETRY_LIMIT {
        assert_eq!(
            state.observe(cts_timeout, 3, mono(0), true, admitted_at_zero),
            Ok(AmpduRetryDecision::RepublishUnchanged { retry_mask: 0b111 })
        );
        assert_eq!(state.protection_failures(), failure);
    }
    assert_eq!(state.current_subframes(), 3);
    assert_eq!(state.acknowledged(), 0);
    assert_eq!(state.block_ack_mpdu_attempts(), 0);
    assert_eq!(state.aggregate_attempts(), 1);
    assert_eq!(
        state.observe(cts_timeout, 3, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::RequestBlockAck {
            retry_mask: 0b111,
            starting_sequence: SequenceNumber::new(40).unwrap(),
        })
    );
    // The request ended that frame-exchange sequence; a later protection
    // failure starts a new count.
    assert_eq!(state.protection_failures(), 0);
    assert_eq!(state.current_subframes(), 3);
}

fn block_ack(starting_sequence: u16, bitmap: u64) -> oer_ieee80211_port::BlockAckReport {
    oer_ieee80211_port::BlockAckReport::new(SequenceNumber::new(starting_sequence).unwrap(), bitmap)
}

fn exhaust_protection(state: &mut AmpduRetryState<32>, first: u16, subframes: u8) {
    let cts_timeout = completion(2, first, 0);
    for _ in 1..VENDOR_SHORT_RETRY_LIMIT {
        state
            .observe(cts_timeout, subframes, mono(0), true, admitted_at_zero)
            .unwrap();
    }
    assert!(matches!(
        state.observe(cts_timeout, subframes, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::RequestBlockAck { .. })
    ));
}

#[test]
fn block_ack_request_answer_resorts_the_kept_aggregate() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(40).unwrap(), 4, HT_POLICY).unwrap();
    exhaust_protection(&mut state, 40, 4);
    // The receiver already holds 40 and 42 from an earlier exchange.
    assert_eq!(
        state
            .observe_block_ack_request(Some(block_ack(40, 0b0101)), mono(0), true, admitted_at_zero)
            .unwrap(),
        AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 }
    );
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.current_subframes(), 2);
    assert_eq!(
        state.current_first_sequence(),
        SequenceNumber::new(41).unwrap()
    );
}

#[test]
fn unanswered_block_ack_request_keeps_every_mpdu_missing() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(7).unwrap(), 3, HT_POLICY).unwrap();
    exhaust_protection(&mut state, 7, 3);
    assert_eq!(
        state
            .observe_block_ack_request(None, mono(0), true, admitted_at_zero)
            .unwrap(),
        AmpduRetryDecision::RetainAggregate { retry_mask: 0b111 }
    );
    assert_eq!(state.acknowledged(), 0);
}

#[test]
fn block_ack_request_answer_after_the_agreement_ended_unaggregates() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(7).unwrap(), 3, HT_POLICY).unwrap();
    exhaust_protection(&mut state, 7, 3);
    assert_eq!(
        state
            .observe_block_ack_request(Some(block_ack(7, 0b001)), mono(0), false, admitted_at_zero)
            .unwrap(),
        AmpduRetryDecision::Unaggregate { retry_mask: 0b110 }
    );
}

#[test]
fn aged_aggregate_ends_at_its_block_ack_request_answer() {
    let mut state =
        AmpduRetryState::<32>::new(SequenceNumber::new(7).unwrap(), 3, HT_POLICY).unwrap();
    exhaust_protection(&mut state, 7, 3);
    assert_eq!(
        state
            .observe_block_ack_request(
                None,
                mono(u64::from(VENDOR_AMPDU_MSDU_LIFETIME_MICROS)),
                true,
                admitted_at_zero
            )
            .unwrap(),
        // Aged under the agreement: a BlockAckReq moves the window past all.
        AmpduRetryDecision::DiscardAged {
            retry_mask: 0,
            aged_mask: 0b111,
            starting_sequence: SequenceNumber::new(10).unwrap(),
        }
    );
}

#[test]
fn advanced_block_ack_ssn_completes_preceding_mpdu_without_retry() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(0, 102, 0), 2, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.aggregate_attempts(), 1);
}

#[test]
fn ack_timeout_completion_applies_a_received_block_ack_bitmap() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(
            completion(5, 20, u64::MAX),
            2,
            mono(0),
            true,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 2);
}

#[test]
fn missing_block_ack_result_ignores_a_stale_success_bitmap() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    let mut stale = completion(5, 20, u64::MAX);
    stale.block_ack_received = false;
    assert_eq!(
        state.observe(stale, 2, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
    );
    assert_eq!(state.acknowledged(), 0);
}

#[test]
fn ht_unaggregates_one_missing_mpdu_but_he_retains_it() {
    let mut ht = AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        ht.observe(completion(0, 20, 0b01), 2, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b10 })
    );

    let mut he = AmpduRetryState::<4>::new(
        SequenceNumber::new(20).unwrap(),
        2,
        AmpduRetryPolicy {
            retain_single_mpdu: true,
            ..HT_POLICY
        },
    )
    .unwrap();
    assert_eq!(
        he.observe(completion(0, 20, 0b01), 2, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
    );
    assert_eq!(he.current_subframes(), 1);
}

#[test]
fn missing_mpdus_are_retried_until_their_lifetime_leaves_less_than_one_unit() {
    let admitted = 5_000;
    let at_admission = |_: u8| Some(mono(admitted));
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 2, HT_POLICY).unwrap();
    // No publication count ends the aggregate: only its MSDU lifetime does.
    let last_live = admitted + u64::from(VENDOR_AMPDU_MSDU_LIFETIME_MICROS) - 1_024;
    for now in [admitted + 1, admitted + 1_000_000, last_live] {
        assert_eq!(
            state.observe(completion(5, 100, 0), 2, mono(now), true, at_admission),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert!(!state.aged(mono(admitted), mono(now)));
    }
    assert_eq!(state.aggregate_attempts(), 4);
    assert!(state.aged(mono(admitted), mono(last_live + 1)));
    // Aged under the agreement: discarded behind a BlockAckReq after them.
    assert_eq!(
        state.observe(
            completion(5, 100, 0),
            2,
            mono(last_live + 1),
            true,
            at_admission
        ),
        Ok(AmpduRetryDecision::DiscardAged {
            retry_mask: 0,
            aged_mask: 0b11,
            starting_sequence: SequenceNumber::new(102).unwrap(),
        })
    );
    assert_eq!(state.discarded_original_indices(), 0b11);
}

#[test]
fn each_mpdu_ages_from_its_own_admission() {
    // 100 waited in software longer than 101.
    let admission = |index: u8| Some(mono(if index == 0 { 0 } else { 500_000 }));
    let aged_head = u64::from(VENDOR_AMPDU_MSDU_LIFETIME_MICROS);
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(5, 100, 0), 2, mono(aged_head), true, admission),
        Ok(AmpduRetryDecision::DiscardAged {
            retry_mask: 0b10,
            aged_mask: 0b01,
            starting_sequence: SequenceNumber::new(101).unwrap(),
        })
    );
    assert_eq!(
        state.block_ack_request_start(),
        SequenceNumber::new(101).unwrap()
    );
    assert_eq!(state.current_subframes(), 1);
    // Without the agreement nothing goes to the window: 101 leaves alone.
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(5, 100, 0), 2, mono(aged_head), false, admission),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b10 })
    );
    // An MPDU without an admission instant is refused, not guessed.
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(5, 100, 0), 2, mono(1), true, |_| None),
        Err(AmpduRetryError::UnknownSubframe { index: 0 })
    );
}

#[test]
fn construction_and_dma_count_disagreements_fail_closed() {
    assert!(matches!(
        AmpduRetryState::<33>::new(SequenceNumber::new(0).unwrap(), 1, HT_POLICY),
        Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: 33 })
    ));
    assert!(matches!(
        AmpduRetryState::<2>::new(SequenceNumber::new(0).unwrap(), 3, HT_POLICY),
        Err(AmpduRetryError::AggregateExceedsCapacity { .. })
    ));
    let mut state =
        AmpduRetryState::<2>::new(SequenceNumber::new(0).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(0, 0, 0b11), 1, mono(0), true, admitted_at_zero),
        Err(AmpduRetryError::FrameCountChanged {
            expected: 2,
            observed: 1
        })
    );
}

#[test]
fn vendor_trigger_timeout_finishes_without_fabricating_block_ack() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    let trigger_timeout = completion_with_tx(
        model_completion(TxCookie(1), TxCompletion::ACK_TIMEOUT_STATUS, 0)
            .with_trigger_flow_model(true),
        20,
        u64::MAX,
    );

    assert_eq!(
        state.observe(trigger_timeout, 2, mono(0), true, admitted_at_zero),
        Ok(AmpduRetryDecision::FinishTriggerFlow)
    );
    assert_eq!(state.trigger_flow_completions(), 1);
    assert_eq!(state.acknowledged(), 0);
    assert_eq!(state.block_ack_mpdu_attempts(), 0);
    assert_eq!(state.aggregate_attempts(), 1);
}

#[test]
fn trigger_timeout_with_reported_packets_stays_on_retry_path() {
    for (primary, last_tx_was_trigger_based, secondary) in
        [(1, false, 0), (0, true, 1), (1, true, 0)]
    {
        let mut state =
            AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
        let completion = completion_with_tx(
            model_completion(TxCookie(1), TxCompletion::ACK_TIMEOUT_STATUS, 0)
                .with_trigger_flow_model(true)
                .with_trigger_packet_counts_model(primary, last_tx_was_trigger_based, secondary),
            20,
            u64::MAX,
        );
        assert_eq!(
            state.observe(completion, 2, mono(0), true, admitted_at_zero),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert_eq!(state.trigger_flow_completions(), 0);
        assert_eq!(state.block_ack_mpdu_attempts(), 2);
    }
}

#[test]
fn trigger_success_predicate_rejects_wrong_status_or_queue_state() {
    for (status, trigger_flow) in [(0, true), (4, true), (5, false)] {
        let mut state =
            AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
        let completion = completion_with_tx(
            model_completion(TxCookie(1), status, 0).with_trigger_flow_model(trigger_flow),
            20,
            0,
        );
        assert_ne!(
            state.observe(completion, 2, mono(0), true, admitted_at_zero),
            Ok(AmpduRetryDecision::FinishTriggerFlow)
        );
    }
}

#[test]
fn an_aggregate_without_any_block_ack_ends_on_the_vendor_retry_limit() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 3, HT_POLICY).unwrap();
    let timeout = HtAmpduTxCompletion {
        block_ack_received: false,
        ..completion(5, 100, 0)
    };
    for attempt in 1..u32::from(VENDOR_SHORT_RETRY_LIMIT) {
        assert_eq!(
            state.observe(timeout, 3, mono(u64::from(attempt)), true, admitted_at_zero),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b111 }),
            "timeout {attempt}"
        );
    }
    assert_eq!(
        state.observe(
            timeout,
            3,
            mono(u64::from(VENDOR_SHORT_RETRY_LIMIT)),
            true,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b111 })
    );
}

#[test]
fn an_aged_aggregate_without_any_block_ack_ends_before_the_retry_limit() {
    let mut state =
        AmpduRetryState::<4>::new(SequenceNumber::new(100).unwrap(), 3, HT_POLICY).unwrap();
    let timeout = HtAmpduTxCompletion {
        block_ack_received: false,
        ..completion(5, 100, 0)
    };
    assert_eq!(
        state.observe(
            timeout,
            3,
            mono(u64::from(VENDOR_AMPDU_MSDU_LIFETIME_MICROS)),
            true,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::DiscardAged {
            retry_mask: 0,
            aged_mask: 0b111,
            starting_sequence: SequenceNumber::new(103).unwrap(),
        })
    );
}

#[test]
fn an_ended_agreement_sends_the_live_missing_mpdus_individually() {
    let he_policy = AmpduRetryPolicy {
        retain_single_mpdu: true,
        ..HT_POLICY
    };
    for policy in [HT_POLICY, he_policy] {
        // Partial BlockAck: the two missing MPDUs leave the aggregate.
        let mut partial =
            AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 3, policy).unwrap();
        assert_eq!(
            partial.observe(
                completion(0, 20, 0b010),
                3,
                mono(0),
                false,
                admitted_at_zero
            ),
            Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b101 })
        );
        assert_eq!(partial.aggregate_attempts(), 1);

        // No BlockAck at all: every MPDU leaves the aggregate.
        let mut timeout =
            AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 3, policy).unwrap();
        let no_block_ack = HtAmpduTxCompletion {
            block_ack_received: false,
            ..completion(5, 20, 0)
        };
        assert_eq!(
            timeout.observe(no_block_ack, 3, mono(0), false, admitted_at_zero),
            Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b111 })
        );
    }
}

#[test]
fn an_ended_agreement_still_discards_aged_or_completes_acknowledged_mpdus() {
    let mut aged =
        AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    let expired = u64::from(VENDOR_AMPDU_MSDU_LIFETIME_MICROS);
    assert_eq!(
        aged.observe(
            completion(0, 20, 0),
            2,
            mono(expired),
            false,
            admitted_at_zero
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
    );

    let mut acknowledged =
        AmpduRetryState::<4>::new(SequenceNumber::new(20).unwrap(), 2, HT_POLICY).unwrap();
    assert_eq!(
        acknowledged.observe(completion(0, 20, 0b11), 2, mono(0), false, admitted_at_zero),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
}

fn mono(micros: u64) -> oer_time::Instant {
    oer_time::Instant::from_micros(micros)
}

/// Every MPDU's MSDU admitted at zero.
fn admitted_at_zero(_: u8) -> Option<oer_time::Instant> {
    Some(oer_time::Instant::from_micros(0))
}

#[test]
fn each_queue_draw_uses_its_head_vif_and_retains_the_complete_record() {
    use oer_ieee80211_port::EdcaParameterSet;
    use oer_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
    let table = |aifsn, min, max, txop, lifetime| {
        EdcaParameterSet::from_tu(
            WmmParameterSet::new(
                0,
                false,
                [WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: oer_wire::U8::new(aifsn).unwrap(),
                    ecw_min: oer_wire::U8::new(min).unwrap(),
                    ecw_max: oer_wire::U8::new(max).unwrap(),
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
            oer_time::Duration::from_micros(100 * 1024)
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
