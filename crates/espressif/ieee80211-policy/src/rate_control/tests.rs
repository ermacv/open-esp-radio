use super::*;
use crate::rate_schedule::{RateScheduleKind, RateScheduleRef};

fn tally(attempted: u16, acknowledged: u16) -> BlockAckTally {
    BlockAckTally::new(NonZeroU16::new(attempted).unwrap(), acknowledged).unwrap()
}

fn state() -> RateControlState {
    RateControlState {
        retry_pressure: 4,
        weighted_retries: 10,
        transmissions: 20,
        completed: 30,
        reevaluate_after_us: 1,
        retry_state_1d: 2,
        retry_state_1e: 3,
        maximum_schedule_index: 5,
        current_schedule: RateScheduleState {
            reference: RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap(),
            retry_limit: 7,
            adaptive: 1,
        },
        legacy_schedule: RateScheduleRef::new(RateScheduleKind::Dot11B, 0).unwrap(),
    }
}

#[test]
fn ack_snr_filter_matches_signed_blob_rounding() {
    let mut filter = AckSnrFilter::new();
    assert_eq!(filter.latest(), None);
    assert_eq!(filter.filtered(), None);

    filter.update(AckSnrFilter::UNINITIALIZED);
    assert_eq!(filter, AckSnrFilter::new());

    // The first valid sample is retained, while the first midpoint is
    // exactly zero because the preceding sample was the sentinel.
    filter.update(-21);
    assert_eq!(filter.latest(), Some(-21));
    assert_eq!(filter.filtered(), Some(0));

    // (-21 + -22) >> 1 is -22 (arithmetic shift), then
    // (3 * 0 + -22) / 4 is -5 (signed division toward zero).
    filter.update(-22);
    assert_eq!(filter.latest(), Some(-22));
    assert_eq!(filter.filtered(), Some(-5));

    // (-22 + 19) >> 1 is -2; (-15 + -2) / 4 is -4.
    filter.update(19);
    assert_eq!(filter.latest(), Some(19));
    assert_eq!(filter.filtered(), Some(-4));
}

#[test]
fn association_installs_selected_schedule_as_owned_runtime_state() {
    let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(70),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.current_schedule(),
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 1).unwrap()
    );
    assert_eq!(
        association.runtime.current_schedule.retry_limit,
        schedule_state(association.current_schedule()).retry_limit
    );

    association.runtime.retry_pressure = 6;
    let update = association.update_tx_per(5);
    assert_eq!(
        update.schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap())
    );
    assert_eq!(
        association.current_schedule(),
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap()
    );
}

#[test]
fn retry_bands_match_the_pinned_transition() {
    let mut low = state();
    assert_eq!(low.update_tx_per(2).schedule, ScheduleSelection::Unchanged);
    assert_eq!(low.retry_pressure, 0);
    assert_eq!(low.transmissions, 21);
    assert_eq!(low.weighted_retries, 13);

    let mut middle = state();
    middle.update_tx_per(4);
    assert_eq!(middle.retry_pressure, 4);

    let mut high = state();
    high.update_tx_per(6);
    assert_eq!(high.retry_pressure, 5);

    let mut very_high = state();
    very_high.update_tx_per(8);
    assert_eq!(very_high.retry_pressure, 6);
    // retry_limit < retries selects retry_limit + 2, not retries + 1.
    assert_eq!(very_high.weighted_retries, 19);
}

#[test]
fn large_counters_are_rescaled_before_accumulation() {
    let mut value = state();
    value.transmissions = 0x01ff_ffff;
    value.weighted_retries = 100;
    value.update_tx_per(0);
    assert_eq!(value.transmissions, 0x0100_0000);
    assert_eq!(value.weighted_retries, 51);
}

#[test]
fn pressure_threshold_clears_state_and_advances_schedule() {
    let mut value = state();
    value.retry_pressure = 6;
    let update = value.update_tx_per(5);
    assert_eq!(
        update.schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11N, 3).unwrap())
    );
    assert_eq!(value.retry_pressure, 0);
    assert_eq!(value.weighted_retries, 0);
    assert_eq!(value.transmissions, 0);
    assert_eq!(value.completed, 0);
    assert_eq!(value.reevaluate_after_us, 500_000);
    assert_eq!(value.retry_state_1d, 0);
    assert_eq!(value.retry_state_1e, 0);
    assert_eq!(value.current_schedule.adaptive, 0);
}

#[test]
fn last_schedule_falls_back_to_the_legacy_table() {
    let mut value = state();
    value.retry_pressure = 6;
    value.maximum_schedule_index = 2;
    value.current_schedule.reference = RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap();
    assert_eq!(
        value.update_tx_per(5).schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11B, 2).unwrap())
    );
}

#[test]
fn invalid_schedule_transition_is_explicit() {
    let mut value = state();
    value.retry_pressure = 6;
    value.maximum_schedule_index = 6;
    value.current_schedule.reference = RateScheduleRef::new(RateScheduleKind::Dot11N, 13).unwrap();
    assert_eq!(value.update_tx_per(5).schedule, ScheduleSelection::Invalid);
}

#[test]
fn byte_pressure_wrap_is_preserved() {
    let mut value = state();
    value.retry_pressure = 0xff;
    value.update_tx_per(8);
    assert_eq!(value.retry_pressure, 1);
}

#[test]
fn beamforming_policy_has_three_exact_modes() {
    assert_eq!(
        beamforming_report_rate(40, 20, true, true),
        BeamformingReportRate {
            mode: 1,
            rate: 16,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    );
    assert_eq!(
        beamforming_report_rate(20, 20, true, true),
        BeamformingReportRate {
            mode: 2,
            rate: 16,
            dcm: true,
            ersu: true,
            ersu_ack: true,
        }
    );
    assert_eq!(
        beamforming_report_rate(20, 20, true, false),
        BeamformingReportRate {
            mode: 0,
            rate: 11,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    );
}

fn selection_input(phy_type: u8) -> PhyModeSelectionInput {
    PhyModeSelectionInput {
        phy_type,
        he_type: 0,
        metric: 20,
        p2p: false,
        supplied_highest_rate: 0,
        use_supplied_highest_rate: false,
        long_range_rates_present: false,
    }
}

#[test]
fn highest_rate_tables_match_recovered_boundaries() {
    for (phy_type, he_type, rate, supplied, index) in [
        (0, 0, 2, true, 3),
        (0, 0, 22, true, 0),
        (1, 0, 12, true, 7),
        (1, 0, 108, true, 0),
        (2, 0, 13, true, 8),
        (2, 0, 144, true, 0),
        (2, 7, 17, true, 9),
        (2, 7, 229, true, 0),
        (2, 0, 0, false, 1),
        (3, 0, 0, false, 0),
        (4, 0, 0, false, 1),
    ] {
        let selected = highest_rate_schedule(PhyModeSelectionInput {
            he_type,
            supplied_highest_rate: rate,
            use_supplied_highest_rate: supplied,
            ..selection_input(phy_type)
        });
        assert_eq!(selected.index(), index);
    }
}

#[test]
fn typed_he_peer_maximum_covers_the_complete_mcs0_to_mcs9_table() {
    let expected = [17, 34, 51, 68, 104, 137, 154, 172, 206, 229];
    for (mcs, expected) in expected.into_iter().enumerate() {
        let mcs = HeMcs::new(mcs as u8).unwrap();
        assert_eq!(
            StaRateControlPeerHighestRate::he20_one_spatial_stream(mcs).vendor_half_mbps(),
            expected
        );
    }
}

#[test]
fn ampdu_thresholds_match_the_he_mcs9_oracle_endpoints() {
    assert_eq!(ampdu_rssi_margin(0x19, 75), 32);
    assert_eq!(ampdu_up_threshold(0x19, 75), 89);
    assert_eq!(ampdu_down_threshold(0x19, 75), 82);
    assert_eq!(
        ampdu_rssi_margin(0x23, AckSnrFilter::UNINITIALIZED as u8),
        0
    );
    assert_eq!(
        ampdu_up_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
        121
    );
    assert_eq!(
        ampdu_down_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
        114
    );
}

#[test]
fn ampdu_owner_promotes_after_two_clean_vendor_windows() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 1, 0).unwrap();
    assert_eq!(
        state.current_schedule(),
        literal_schedule!(RateScheduleKind::Dot11Ax, 1)
    );
    assert_eq!(
        state.observe_block_ack(600_000, tally(500, 500), Some(75)),
        AmpduRateDecision::Retain {
            raw_success_ratio: 128,
            filtered_success_ratio: 110,
        }
    );
    assert_eq!(
        state.observe_block_ack(700_001, tally(500, 500), Some(75)),
        AmpduRateDecision::Promote {
            from: literal_schedule!(RateScheduleKind::Dot11Ax, 1),
            to: literal_schedule!(RateScheduleKind::Dot11Ax, 0),
            raw_success_ratio: 128,
            filtered_success_ratio: 114,
        }
    );
    assert_eq!(
        state.current_schedule(),
        literal_schedule!(RateScheduleKind::Dot11Ax, 0)
    );
    assert_eq!(state.filtered_success_ratio(), None);
}

#[test]
fn ampdu_owner_lowers_only_after_two_filtered_bad_windows() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
    assert!(matches!(
        state.observe_block_ack(100_001, tally(500, 0), Some(75)),
        AmpduRateDecision::Retain {
            filtered_success_ratio: 93,
            ..
        }
    ));
    assert!(matches!(
        state.observe_block_ack(200_002, tally(500, 0), Some(75)),
        AmpduRateDecision::Retain {
            filtered_success_ratio: 69,
            ..
        }
    ));
    assert_eq!(
        state.observe_block_ack(300_003, tally(500, 0), Some(75)),
        AmpduRateDecision::Lower {
            from: literal_schedule!(RateScheduleKind::Dot11Ax, 0),
            to: literal_schedule!(RateScheduleKind::Dot11Ax, 1),
            raw_success_ratio: 0,
            filtered_success_ratio: 51,
        }
    );
}

#[test]
fn ampdu_owner_accepts_checked_counts_without_a_post_mutation_error() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
    assert_eq!(
        state.observe_block_ack(10, tally(16, 16), None),
        AmpduRateDecision::Accumulating
    );
    let decision = state.observe_block_ack(11, tally(u16::MAX, u16::MAX), None);
    assert!(matches!(
        decision,
        AmpduRateDecision::Retain {
            raw_success_ratio: 128,
            ..
        }
    ));
    assert_eq!(vendor_duration(4, u32::MAX - 5), 9);
}

#[test]
fn ampdu_initial_rate_uses_the_vendor_index_eight_floor() {
    let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.current_schedule(),
        literal_schedule!(RateScheduleKind::Dot11Ax, 13)
    );
    assert_eq!(
        association.current_ampdu_schedule(),
        Some(literal_schedule!(RateScheduleKind::Dot11Ax, 8))
    );
}

#[test]
fn phy_mode_selector_covers_legacy_p2p_ht_he_and_lora() {
    let dot11b = select_phy_mode(selection_input(0));
    assert_eq!(
        dot11b.current,
        literal_schedule!(RateScheduleKind::Dot11B, 3)
    );
    assert_eq!(dot11b.maximum_index, 3);
    assert_eq!(dot11b.schedule_count, 6);

    let mut dot11g_input = selection_input(1);
    dot11g_input.metric = 12;
    dot11g_input.p2p = true;
    let dot11g = select_phy_mode(dot11g_input);
    assert_eq!(
        dot11g.current,
        literal_schedule!(RateScheduleKind::P2pDot11G, 5)
    );
    assert_eq!(
        dot11g.secondary,
        literal_schedule!(RateScheduleKind::P2pDot11G, 7)
    );
    assert_eq!(dot11g.maximum_index, 7);

    let mut ht_input = selection_input(2);
    ht_input.metric = 8;
    let ht = select_phy_mode(ht_input);
    assert_eq!(ht.current, literal_schedule!(RateScheduleKind::Dot11N, 11));
    assert_eq!(ht.ampdu_limit_rate, Some(0x10));

    let mut he_input = selection_input(4);
    he_input.he_type = 7;
    he_input.metric = 8;
    he_input.long_range_rates_present = true;
    let he = select_phy_mode(he_input);
    assert_eq!(he.current, literal_schedule!(RateScheduleKind::Dot11Ax, 13));
    assert_eq!(he.maximum_index, 15);
    assert_eq!(he.ampdu_limit_rate, Some(0x12));
    assert_eq!(he.fallback, literal_schedule!(RateScheduleKind::Lora, 1));

    let lora = select_phy_mode(selection_input(6));
    assert_eq!(lora.current, literal_schedule!(RateScheduleKind::Lora, 0));
    assert_eq!(lora.schedule_count, 2);
}

#[test]
fn associated_he_owner_joins_schedule_and_report_rate_without_c_layout() {
    let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(20),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: true,
        he_low_metric_report: HeLowMetricReportFeatures {
            dcm_receive_supported: true,
            extended_range_single_user_permitted: true,
        },
    });

    assert_eq!(
        association.current_schedule(),
        literal_schedule!(RateScheduleKind::Dot11Ax, 7)
    );
    assert_eq!(
        association.fallback_schedule(),
        literal_schedule!(RateScheduleKind::Lora, 1)
    );
    assert_eq!(association.maximum_schedule_index(), 15);
    assert_eq!(association.schedule_count(), 16);
    assert_eq!(association.ampdu_limit_rate(), Some(0x13));
    assert_eq!(
        association.beamforming_report(),
        beamforming_report_rate_for_metric(20, true, true)
    );
}

#[test]
fn associated_he_owner_keeps_low_metric_feature_gates_explicit() {
    let mut input = StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: Some(StaRateControlPeerHighestRate::he20_one_spatial_stream(
            HeMcs::new(9).unwrap(),
        )),
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    };
    let ordinary = StaRateControlAssociation::new(input);
    assert_eq!(
        ordinary.current_schedule(),
        literal_schedule!(RateScheduleKind::Dot11Ax, 0)
    );
    assert_eq!(
        ordinary.beamforming_report(),
        beamforming_report_rate_for_metric(8, false, false)
    );

    input.he_low_metric_report = HeLowMetricReportFeatures {
        dcm_receive_supported: true,
        extended_range_single_user_permitted: true,
    };
    assert_eq!(
        StaRateControlAssociation::new(input).beamforming_report(),
        beamforming_report_rate_for_metric(8, true, true)
    );
}

#[test]
fn sta_link_metric_preserves_the_blob_signed_byte_subtraction() {
    assert_eq!(
        StaLinkMetric::from_rssi_and_noise_floor(-30, -96).value(),
        66
    );
    assert_eq!(
        StaLinkMetric::from_rssi_and_noise_floor(100, -100).value(),
        -56
    );
}

#[test]
fn recovered_rate_callbacks_and_ampdu_table_are_finite() {
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 0), Some(3));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 42), Some(4));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 15), Some(6));
    // Long-range codes select the 802.11g arena's long-range records.
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 0x29), Some(12));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 0x2a), Some(11));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x21), Some(0));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x29), Some(13));
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x23), Some(0));
    assert_eq!(
        rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x2a),
        Some(14)
    );
    assert_eq!(rate_to_schedule_index(RateIndexMap::Lora, 0x28), None);
}

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

#[test]
fn non_data_frames_use_the_association_control_schedule() {
    let basic_ofdm = RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap();
    for phy in [
        StaRateControlPhy::Dot11G,
        StaRateControlPhy::Ht,
        StaRateControlPhy::He,
    ] {
        assert_eq!(association(phy, false).control_schedule(), basic_ofdm);
    }
    assert_eq!(
        association(StaRateControlPhy::Dot11B, false).control_schedule(),
        DEFAULT_CONTROL_SCHEDULE
    );
    assert_eq!(
        association(StaRateControlPhy::Dot11G, true).control_schedule(),
        RateScheduleRef::new(RateScheduleKind::P2pDot11G, 7).unwrap()
    );
    assert_eq!(
        association(StaRateControlPhy::He, true).control_schedule(),
        DEFAULT_CONTROL_SCHEDULE
    );
}

mod seam {
    use oer_ieee80211_lower_mac::{BlockAckSnr, LinkMetric, SnrSample};

    /// BlockAck evidence named by its hardware condition, as the S31's is.
    enum Evidence {
        Measured(i8),
        StatusWithoutSample,
    }

    impl BlockAckSnr for Evidence {
        fn snr_sample(&self) -> SnrSample {
            match *self {
                Self::Measured(snr) => SnrSample::Measured(snr),
                Self::StatusWithoutSample => SnrSample::Unmeasured,
            }
        }
    }
    use oer_ieee80211_mac::{
        ht::ht_peer_capabilities,
        phy::{HtMcs, HtRate, PhyRate, PpduBandwidth},
        station::association::PhyMode,
    };
    use oer_ieee80211_upper_mac::rate_control::{
        AmpduUnavailable, MpduOutcome, RateControl, RatePeer,
    };

    use super::{NonZeroU16, tally};

    use super::super::{EspressifRateControl, RateScheduleKind, RateScheduleRef, schedule_state};

    /// An HT20 peer, with or without the 20 MHz short guard
    /// interval.
    fn ht20_peer(short_gi: bool) -> RatePeer {
        let mut element = [0_u8; 28];
        element[..6].copy_from_slice(&[45, 26, if short_gi { 0x20 } else { 0 }, 0, 0x17, 0xff]);
        RatePeer {
            phy: PhyMode::Ht20,
            legacy_rates:
                oer_ieee80211_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                    &[2, 4, 11, 22, 12, 18, 24, 36],
                    &[48, 72, 96, 108],
                )
                .unwrap(),
            ht_capabilities: ht_peer_capabilities(&element),
            he_capabilities: None,
            he_peer_state: None,
        }
    }

    /// The metric of a frame `db` dB over the noise floor.
    fn metric(db: i8) -> LinkMetric {
        LinkMetric::new(db, 0)
    }

    #[test]
    fn a_weaker_link_starts_at_a_slower_rate() {
        let peer = ht20_peer(true);
        let weakest = EspressifRateControl::for_peer((), &peer, metric(i8::MIN));
        let strong = EspressifRateControl::for_peer((), &peer, metric(60));
        assert!(weakest.mpdu_rate().nominal_kbps() < strong.mpdu_rate().nominal_kbps());
    }

    #[test]
    fn a_short_guard_interval_is_used_only_where_the_peer_supports_it() {
        let with = EspressifRateControl::for_peer((), &ht20_peer(true), metric(60)).decode;
        let without = EspressifRateControl::for_peer((), &ht20_peer(false), metric(60)).decode;
        let mut short = 0;
        for index in 0..14 {
            let Some(schedule) = RateScheduleRef::new(RateScheduleKind::Dot11N, index) else {
                continue;
            };
            let raw = crate::rate_code::phy_rate(
                schedule.kind(),
                schedule_state(schedule).rate,
                PpduBandwidth::Mhz20,
                oer_ieee80211_mac::phy::HeGiLtf::Ltf2xGi800Ns,
            );
            if let Some(raw @ PhyRate::Ht(rate)) = raw {
                assert_eq!(with.rate(schedule), raw);
                let PhyRate::Ht(qualified) = without.rate(schedule) else {
                    panic!("an HT schedule decodes to HT");
                };
                assert!(!qualified.short_gi());
                assert_eq!(qualified.mcs(), rate.mcs());
                short += usize::from(rate.short_gi());
            }
        }
        // The 802.11n schedules do name short-GI rates.
        assert!(short > 0);
        let _ = (HtMcs::new(0), HtRate::new);
    }

    #[test]
    fn retry_pressure_lowers_the_rate() {
        let mut control = EspressifRateControl::for_peer((), &ht20_peer(true), metric(60));
        let before = control.mpdu_rate().nominal_kbps();
        // Exchanges of ten attempts each add two to the retry pressure; past
        // six the controller moves to the next, slower schedule.
        for _ in 0..4 {
            control.observe_mpdu(
                NonZeroU16::new(10).unwrap(),
                MpduOutcome::Unacknowledged,
                None,
            );
        }
        assert!(control.mpdu_rate().nominal_kbps() < before);
    }

    #[test]
    fn a_block_ack_s_ack_snr_feeds_the_filter() {
        let mut control = EspressifRateControl::for_peer((), &ht20_peer(true), metric(60));
        assert_eq!(control.association().filtered_ack_snr(), None);
        assert_eq!(
            control.observe_ampdu(
                oer_time::Instant::from_micros(1_000),
                tally(4, 4),
                &Evidence::Measured(30)
            ),
            Ok(())
        );
        assert!(control.association().filtered_ack_snr().is_some());
    }

    #[test]
    fn a_block_ack_without_a_sample_leaves_the_filter_and_still_counts_its_window() {
        let mut control = EspressifRateControl::for_peer((), &ht20_peer(true), metric(60));
        let now = oer_time::Instant::from_micros(1_000);
        assert_eq!(
            control.observe_ampdu(now, tally(4, 4), &Evidence::Measured(30)),
            Ok(())
        );
        let filtered = control.association().filtered_ack_snr();
        let latest = control.association().latest_ack_snr();
        assert_eq!(
            control.observe_ampdu(now, tally(4, 1), &Evidence::StatusWithoutSample),
            Ok(())
        );
        assert_eq!(control.association().filtered_ack_snr(), filtered);
        assert_eq!(control.association().latest_ack_snr(), latest);
        // A measured BlockAck moves the filter again.
        assert_eq!(
            control.observe_ampdu(now, tally(4, 4), &Evidence::Measured(10)),
            Ok(())
        );
        assert_ne!(control.association().filtered_ack_snr(), filtered);
    }

    #[test]
    fn an_unavailable_aggregate_controller_returns_the_refusal_without_mutation() {
        let peer = RatePeer {
            phy: PhyMode::Legacy,
            legacy_rates:
                oer_ieee80211_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                    &[2, 4, 11, 22, 12, 18, 24, 36],
                    &[48, 72, 96, 108],
                )
                .unwrap(),
            ht_capabilities: None,
            he_capabilities: None,
            he_peer_state: None,
        };
        let mut control = EspressifRateControl::for_peer((), &peer, metric(60));
        let before = control.ampdu_rate();
        assert_eq!(
            control.observe_ampdu(
                oer_time::Instant::from_micros(1_000),
                tally(4, 4),
                &Evidence::Measured(30)
            ),
            Err(AmpduUnavailable)
        );
        assert_eq!(control.association().filtered_ack_snr(), None);
        assert_eq!(control.ampdu_rate(), before);
    }
}

/// Every selection branch retains valid records: all link metrics and both
/// i32 extremes, each PHY arm and the default arm's endpoints, HE/P2P/long
/// range flags, every named peer rate and representatives of unknown rates.
#[test]
fn phy_mode_selection_indices_stay_inside_their_tables() {
    const RATES: [u32; 35] = [
        0,
        2,
        4,
        11,
        22,
        12,
        18,
        24,
        36,
        48,
        72,
        96,
        108,
        13,
        26,
        39,
        52,
        78,
        104,
        117,
        130,
        144,
        17,
        34,
        51,
        68,
        137,
        154,
        172,
        206,
        229,
        1,
        3,
        255,
        u32::MAX,
    ];
    let mut inputs = 0_u32;
    for phy_type in (0..=7).chain([u8::MAX]) {
        for he_type in [0, 7, u8::MAX] {
            for metric in (i8::MIN..=i8::MAX)
                .map(i32::from)
                .chain([i32::MIN, i32::MAX])
            {
                for flags in 0..8_u8 {
                    for supplied_highest_rate in RATES {
                        let selection = select_phy_mode(PhyModeSelectionInput {
                            phy_type,
                            he_type,
                            metric,
                            p2p: flags & 1 != 0,
                            supplied_highest_rate,
                            use_supplied_highest_rate: flags & 2 != 0,
                            long_range_rates_present: flags & 4 != 0,
                        });
                        for schedule in [
                            selection.current,
                            selection.secondary,
                            selection.fallback,
                            selection.legacy,
                        ] {
                            assert!(usize::from(schedule.index()) < schedule.kind().record_count());
                        }
                        inputs += 1;
                    }
                }
            }
        }
    }
    assert_eq!(inputs, 9 * 3 * 258 * 8 * 35);
}

/// In every table the last record's rate is no faster than any other record's.
/// The vendor's Long Range codes have no portable rate and count at their nominal rates,
/// `wifi_phy_rate_t`'s `WIFI_PHY_RATE_LORA_250K` (`0x29`) and
/// `WIFI_PHY_RATE_LORA_500K` (`0x2a`).
#[test]
fn the_last_record_of_every_table_is_the_slowest() {
    const KINDS: [RateScheduleKind; 9] = [
        RateScheduleKind::BarOfdm,
        RateScheduleKind::BasicOfdm,
        RateScheduleKind::Dot11Ax,
        RateScheduleKind::Dot11B,
        RateScheduleKind::Dot11G,
        RateScheduleKind::Dot11N,
        RateScheduleKind::Lora,
        RateScheduleKind::P2pDot11G,
        RateScheduleKind::P2pDot11N,
    ];
    let kbps = |kind, index| {
        let code = schedule_state(RateScheduleRef::new(kind, index).unwrap()).rate;
        match code {
            0x29 => return 250,
            0x2a => return 500,
            _ => {}
        }
        crate::rate_code::phy_rate(
            kind,
            code,
            PpduBandwidth::Mhz20,
            oer_ieee80211_mac::phy::HeGiLtf::Ltf2xGi800Ns,
        )
        .map(|rate| rate.nominal_kbps())
        .unwrap_or_else(|| panic!("{kind:?} record {index} code {code:#x} has no rate"))
    };
    for kind in KINDS {
        let last = (kind.record_count() - 1) as u8;
        for index in 0..last {
            assert!(
                kbps(kind, last) <= kbps(kind, index),
                "{kind:?}: record {index} is slower than the last"
            );
        }
    }
}
