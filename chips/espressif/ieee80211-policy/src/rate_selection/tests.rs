//! The Espressif rate selection against the vendor's records.

extern crate std;

use core::num::NonZeroU16;
use std::vec::Vec;

use oer_ieee80211_port::{
    BandSet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RETRY_STAGES, RateSupport,
    ReceiveFilter, WidthSet,
};
use {
    oer_ieee80211_port::PhyMode,
    oer_ieee80211_wire::{
        ht::ht_peer_capabilities,
        phy::{HeMcs, HtMcs},
    },
};

use super::*;
use crate::rate_schedule::{RateScheduleKind, schedule_state};
use oer_ieee80211_port::{
    Air, IndeterminateCause, SingleEnd, SingleResponse, TxOutcome, WithdrawCause,
};
use oer_ieee80211_port::{Channel, ChannelWidth, LinkMetric, RxMeta, RxMetadata};

struct Caps;

impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_wire::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: true,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: None,
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

/// The metadata of a frame received `db` dB over the noise floor.
#[derive(Clone, Copy)]
struct Measured(i8, RxMeta);

impl RxMetadata for Measured {
    fn common(&self) -> &RxMeta {
        &self.1
    }
}

impl SignalMetadata for Measured {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.0, 0)
    }
}

/// The metadata of an Association Response `db` dB over the noise floor.
fn metric(db: i8) -> Measured {
    Measured(
        db,
        RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()),
    )
}

fn legacy_peer() -> RatePeer {
    RatePeer {
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
    }
}

fn ht20_peer() -> RatePeer {
    let mut element = [0_u8; 28];
    element[..6].copy_from_slice(&[45, 26, 0x20, 0, 0x17, 0xff]);
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

/// The plan of `frame` on a channel that carries every rate.
fn plan_of(selection: &EspressifRateSelection<Caps>, frame: FrameRates) -> RetryPlan<PhyRate> {
    RateSelection::<Measured, Snr>::plan(selection, frame, Some).expect("every rate is carried")
}

fn stages(plan: &RetryPlan<PhyRate>) -> Vec<(PhyRate, u8)> {
    plan.stages()
        .map(|stage| (stage.rate, stage.transmissions.get()))
        .collect()
}

/// The rate of every transmission of a control record, by the vendor's
/// walk: the last rate once the record has no further one.
fn record_walk(schedule: RateScheduleRef) -> Vec<PhyRate> {
    let mut rates = Vec::new();
    let mut last = None;
    for failures in 0..schedule_publication_limit(schedule) {
        let rate = schedule_rate_after_failures(schedule, failures)
            .and_then(legacy_rate)
            .map(PhyRate::Legacy)
            .or(last)
            .unwrap();
        last = Some(rate);
        rates.push(rate);
    }
    rates
}

fn plan_walk(plan: &RetryPlan<PhyRate>) -> Vec<PhyRate> {
    stages(plan)
        .into_iter()
        .flat_map(|(rate, transmissions)| core::iter::repeat_n(rate, usize::from(transmissions)))
        .collect()
}

/// ACK evidence that carries the response's SNR, as the S31's does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Snr(i8);

impl AckSnr for Snr {
    fn snr_db(&self) -> i8 {
        self.0
    }
}

type Outcome = SingleOutcome<Snr, ()>;

fn air(transmissions: u16) -> Air {
    Air {
        transmissions: NonZeroU16::new(transmissions).unwrap(),
        rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
    }
}

fn delivered(transmissions: u16) -> Outcome {
    TxOutcome::Ended(SingleEnd::Delivered {
        air: air(transmissions),
        response: SingleResponse::Ack(Snr(30)),
    })
}

fn ack_timeout(transmissions: u16) -> Outcome {
    TxOutcome::Ended(SingleEnd::AckTimeout(air(transmissions)))
}

#[test]
fn a_probe_goes_once_at_one_megabit() {
    let selection = EspressifRateSelection::<Caps>::new();
    assert_eq!(
        stages(&plan_of(&selection, FrameRates::Probe)),
        [(PhyRate::Legacy(LegacyRate::Dsss1M), 1)]
    );
}

#[test]
fn until_associated_control_frames_walk_the_default_record() {
    let selection = EspressifRateSelection::<Caps>::new();
    let plan = plan_of(&selection, FrameRates::Control);
    assert_eq!(plan_walk(&plan), record_walk(DEFAULT_CONTROL_SCHEDULE));
    assert_eq!(
        plan.stages().next().unwrap().rate,
        PhyRate::Legacy(LegacyRate::Dsss1M)
    );
    assert_eq!(
        plan.transmissions(),
        u16::from(schedule_publication_limit(DEFAULT_CONTROL_SCHEDULE))
    );
}

#[test]
fn associated_control_frames_walk_the_association_record_and_leaving_restores_the_default() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured, Snr>::associated(&mut selection, &legacy_peer(), &metric(40));
    let schedule = selection
        .control()
        .unwrap()
        .association()
        .control_schedule();
    // An 802.11g association: the vendor's BasicOFDMSched, from 6 Mb/s.
    assert_eq!(
        schedule,
        RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap()
    );
    let plan = plan_of(&selection, FrameRates::Control);
    assert_eq!(plan_walk(&plan), record_walk(schedule));
    assert_eq!(
        plan.stages().next().unwrap().rate,
        PhyRate::Legacy(LegacyRate::Ofdm6M)
    );
    RateSelection::<Measured, Snr>::disconnected(&mut selection);
    assert_eq!(
        plan_walk(&plan_of(&selection, FrameRates::Control)),
        record_walk(DEFAULT_CONTROL_SCHEDULE)
    );
}

#[test]
fn data_starts_at_the_controls_rate_and_steps_down_the_vendor_ladder_within_its_budget() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht20_peer(), &metric(60));
    let control = selection.control().unwrap();
    let initial = control.mpdu_rate();
    let limit = schedule_publication_limit(control.association().current_schedule());
    let plan = plan_of(&selection, FrameRates::Data);
    let walk = plan_walk(&plan);
    assert_eq!(walk.len(), usize::from(limit.max(1)));
    assert_eq!(walk[0], initial);
    let mut last = initial;
    for (failures, rate) in walk.iter().enumerate().skip(1) {
        let expected = ordinary_retry_rate(initial, failures as u8).unwrap_or(last);
        assert_eq!(*rate, expected, "transmission {failures}");
        last = expected;
    }
    assert!(plan.stages().count() <= RETRY_STAGES);
}

#[test]
fn data_completions_adapt_the_rate_as_the_vendor_control_does() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    let mut reference = EspressifRateControl::for_peer((), &ht20_peer(), LinkMetric::new(60, 0));
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht20_peer(), &metric(60));
    let before = plan_of(&selection, FrameRates::Data)
        .stages()
        .next()
        .unwrap()
        .rate;
    for _ in 0..64 {
        RateSelection::<Measured, Snr>::completed(
            &mut selection,
            Instant::from_micros(0),
            FrameRates::Data,
            &ack_timeout(8),
        );
        reference.observe_mpdu(
            NonZeroU16::new(8).unwrap(),
            MpduOutcome::Unacknowledged,
            None,
        );
        // Control frames and ones that never went teach nothing.
        RateSelection::<Measured, Snr>::completed(
            &mut selection,
            Instant::from_micros(0),
            FrameRates::Control,
            &ack_timeout(8),
        );
        RateSelection::<Measured, Snr>::completed(
            &mut selection,
            Instant::from_micros(0),
            FrameRates::Data,
            &Outcome::Withdrawn(WithdrawCause::GateClosed),
        );
    }
    let after = plan_of(&selection, FrameRates::Data)
        .stages()
        .next()
        .unwrap()
        .rate;
    assert_eq!(after, reference.mpdu_rate());
    assert_eq!(
        selection
            .control()
            .unwrap()
            .association()
            .current_schedule(),
        reference.association().current_schedule()
    );
    assert!(
        after.nominal_kbps() < before.nominal_kbps(),
        "{before:?} -> {after:?}"
    );
}

#[test]
fn the_control_records_a_station_uses_start_with_a_legacy_rate() {
    for schedule in [
        DEFAULT_CONTROL_SCHEDULE,
        RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap(),
    ] {
        assert!(
            legacy_rate(schedule_state(schedule).rate).is_some(),
            "{schedule:?}"
        );
    }
}

/// An HT40 peer: what its association decodes at 40 MHz.
fn ht40_peer() -> RatePeer {
    let mut element = [0_u8; 28];
    element[..6].copy_from_slice(&[45, 26, 0x02, 0, 0x17, 0xff]);
    RatePeer {
        phy: PhyMode::Ht40,
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

#[test]
fn on_a_20_mhz_channel_an_ht40_records_plan_keeps_its_20_mhz_stages() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht40_peer(), &metric(60));
    let vendor = plan_of(&selection, FrameRates::Data);
    assert!(
        vendor
            .stages()
            .any(|stage| stage.rate.bandwidth().mhz() == 40),
        "the record starts at HT40: {vendor:?}"
    );
    // A 20 MHz channel carries no 40 MHz rate: those stages are left out,
    // the rest kept in order, without a panic.
    let carried = |rate: PhyRate| (rate.bandwidth().mhz() <= 20).then_some(rate);
    let on_channel = RateSelection::<Measured, Snr>::plan(&selection, FrameRates::Data, carried)
        .expect("the ladder steps down to 20 MHz rates");
    let expected: Vec<_> = vendor
        .stages()
        .filter(|stage| stage.rate.bandwidth().mhz() <= 20)
        .map(|stage| (stage.rate, stage.transmissions.get()))
        .collect();
    assert_eq!(stages(&on_channel), expected);
    // A channel that carries none of them: the frame does not go.
    assert!(
        RateSelection::<Measured, Snr>::plan(&selection, FrameRates::Data, |_| None::<PhyRate>)
            .is_none()
    );
}

#[test]
fn an_attempt_cut_before_the_air_leaves_the_rate_control_as_it_was() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht20_peer(), &metric(60));
    let state = |selection: &EspressifRateSelection<Caps>| {
        std::format!("{:?}", selection.control().unwrap().association())
    };
    let before = state(&selection);
    // Aborted before its first transmission, as a gate closing for an
    // absence aborts it: no sample.
    RateSelection::<Measured, Snr>::completed(
        &mut selection,
        Instant::from_micros(0),
        FrameRates::Data,
        &Outcome::Withdrawn(WithdrawCause::GateClosed),
    );
    assert_eq!(state(&selection), before);
    // One transmission on the air is a sample.
    RateSelection::<Measured, Snr>::completed(
        &mut selection,
        Instant::from_micros(0),
        FrameRates::Data,
        &delivered(1),
    );
    assert_ne!(state(&selection), before);
}

#[test]
fn unknown_outcomes_and_contention_exhaustion_never_change_rate_state() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht20_peer(), &metric(60));
    let before = std::format!("{:?}", selection.control().unwrap().association());
    for outcome in [
        Outcome::Indeterminate(IndeterminateCause::HardwareTimeout),
        Outcome::Withdrawn(WithdrawCause::KeyRemoved),
        TxOutcome::Ended(SingleEnd::ContentionLimit(None)),
        TxOutcome::Ended(SingleEnd::ContentionLimit(Some(air(3)))),
        TxOutcome::Ended(SingleEnd::CtsTimeout(None)),
        TxOutcome::Ended(SingleEnd::AirDenied {
            denial: (),
            air: Some(air(2)),
        }),
    ] {
        assert!(outcome.rate_sample().is_none());
        for _ in 0..64 {
            RateSelection::<Measured, Snr>::completed(
                &mut selection,
                Instant::from_micros(0),
                FrameRates::Data,
                &outcome,
            );
        }
        assert_eq!(
            std::format!("{:?}", selection.control().unwrap().association()),
            before
        );
    }
}

#[test]
fn an_unrepresentable_walk_never_publishes_a_partial_retry_budget() {
    let initial = PhyRate::Legacy(LegacyRate::Dsss1M);
    let budget = 5;
    let plan = walk(initial, budget, |failed| {
        Some(PhyRate::Legacy(if failed.is_multiple_of(2) {
            LegacyRate::Dsss1M
        } else {
            LegacyRate::Ofdm6M
        }))
    });
    assert!(
        plan.is_none_or(|plan| plan.transmissions() == u16::from(budget)),
        "a fifth rate stage must refuse the whole plan, not shorten its budget"
    );
}

#[test]
fn a_zero_retry_budget_never_creates_a_transmission() {
    let rate = PhyRate::Legacy(LegacyRate::Dsss1M);
    assert!(walk(rate, 0, |_| Some(rate)).is_none());
}

#[test]
fn every_data_and_control_rate_belongs_to_the_negotiated_legacy_set() {
    use oer_ieee80211_upper_mac::rate_control::NegotiatedLegacyRates;
    let he = RatePeer {
        phy: PhyMode::He20,
        he_capabilities: Some(
            oer_ieee80211_wire::he::parse_he20_capabilities(&[
                255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d,
                0xc0, 0x1f, 0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
            ])
            .unwrap(),
        ),
        ..ht20_peer()
    };
    for (supported, extended) in [
        (&[0x82, 0x84, 0x8b, 0x96][..], &[][..]),
        (&[0x82, 0x84][..], &[][..]),
        (&[0x82, 0x8b][..], &[0x0c, 0x30][..]),
        (&[0x8c][..], &[][..]),
        (
            &[0x82, 0x84, 0x8b, 0x96][..],
            &[12, 18, 24, 36, 48, 72, 96, 108][..],
        ),
    ] {
        for mut peer in [legacy_peer(), ht20_peer(), ht40_peer(), he] {
            peer.legacy_rates =
                NegotiatedLegacyRates::from_rate_elements(supported, extended).unwrap();
            for signal in [-20, 0, 20, 60, 100] {
                let mut selection = EspressifRateSelection::<Caps>::new();
                RateSelection::<Measured, Snr>::associated(&mut selection, &peer, &metric(signal));
                for _ in 0..32 {
                    for class in [FrameRates::Control, FrameRates::Data] {
                        let plan = plan_of(&selection, class);
                        for (rate, _) in stages(&plan) {
                            if let PhyRate::Legacy(rate) = rate {
                                assert!(
                                    peer.legacy_rates.contains(rate),
                                    "{peer:?}, {class:?}: {rate:?}"
                                );
                            } else {
                                assert_ne!(peer.phy, PhyMode::Legacy);
                            }
                        }
                    }
                    RateSelection::<Measured, Snr>::completed(
                        &mut selection,
                        Instant::from_micros(1),
                        FrameRates::Data,
                        &ack_timeout(32),
                    );
                }
            }
        }
    }
}

#[test]
fn a_delivered_outcome_feeds_its_ack_snr_to_the_rate_control() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    let mut reference = EspressifRateControl::for_peer((), &ht20_peer(), LinkMetric::new(60, 0));
    RateSelection::<Measured, Snr>::associated(&mut selection, &ht20_peer(), &metric(60));
    for snr in [30, 18, 25] {
        RateSelection::<Measured, Snr>::completed(
            &mut selection,
            Instant::from_micros(0),
            FrameRates::Data,
            &TxOutcome::Ended(SingleEnd::<Snr, ()>::Delivered {
                air: air(2),
                response: SingleResponse::Ack(Snr(snr)),
            }),
        );
        reference.observe_mpdu(
            NonZeroU16::new(2).unwrap(),
            MpduOutcome::Acknowledged,
            Some(snr),
        );
    }
    let association = selection.control().unwrap().association();
    assert_eq!(association.latest_ack_snr(), Some(25));
    assert_eq!(
        association.filtered_ack_snr(),
        reference.association().filtered_ack_snr()
    );
    assert!(association.filtered_ack_snr().is_some());
}
