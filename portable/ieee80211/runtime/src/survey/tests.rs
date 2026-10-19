use super::{GAP_LOG_CAPACITY, Gap, MinMax, Survey, missed_beacons};
use core::num::NonZeroU64;
use oer_base_time::Instant;
use oer_ieee80211_lower_mac::{Channel, ChannelWidth, RxEvidence, RxMeta};
use oer_ieee80211_mac::address::{Bssid, IndividualAddress};
use std::vec::Vec;

const STAND: [u8; 6] = [2, 0, 0, 0, 0, 1];
const OTHER: [u8; 6] = [2, 0, 0, 0, 0, 2];
/// 100 TU in µs.
const BI: u64 = 102_400;

fn bssid(address: [u8; 6]) -> Bssid {
    Bssid::new(IndividualAddress::new(address).unwrap()).unwrap()
}

/// A beacon (subtype 8) or probe response (subtype 5) from `bss` with its
/// TSF, beacon interval and an optional DS Parameter Set.
fn frame(subtype: u8, bss: [u8; 6], tsf: u64, interval_tu: u16, ds: Option<u8>) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 36];
    frame[0] = subtype << 4;
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&bss);
    frame[16..22].copy_from_slice(&bss);
    frame[24..32].copy_from_slice(&tsf.to_le_bytes());
    frame[32..34].copy_from_slice(&interval_tu.to_le_bytes());
    frame[34] = 0x01;
    frame.extend_from_slice(&[0, 5, b's', b't', b'a', b'n', b'd']);
    if let Some(channel) = ds {
        frame.extend_from_slice(&[3, 1, channel]);
    }
    frame
}

fn beacon(bss: [u8; 6], tsf: u64) -> Vec<u8> {
    frame(8, bss, tsf, 100, None)
}

fn meta(rssi: RxEvidence<i8>) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
    meta.rssi_dbm = rssi;
    meta
}

fn heard(rssi: i8) -> RxMeta {
    meta(RxEvidence::HardwareObserved(rssi))
}

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

/// Take `frame` at stamp `stamp`, handed off 50 µs later.
fn take<const N: usize>(survey: &mut Survey<N>, frame: &[u8], stamp: u32) {
    let handoff = at(u64::from(stamp) + 50);
    survey.take(handoff, frame, &heard(-40), Some(stamp), handoff);
}

#[test]
fn beacons_and_probe_responses_are_tallied_under_their_table_index() {
    let mut survey = Survey::<4>::new(bssid(STAND));
    take(&mut survey, &beacon(STAND, 0), 0);
    take(&mut survey, &frame(5, OTHER, 7, 100, None), 10);
    take(&mut survey, &beacon(STAND, BI), 102_400);
    take(&mut survey, &frame(5, STAND, BI + 9, 100, None), 102_500);

    let (record, stand) = survey.bss(0).unwrap();
    assert_eq!(record.bssid, bssid(STAND));
    assert_eq!((stand.beacons, stand.probe_responses), (2, 1));
    let (record, other) = survey.bss(1).unwrap();
    assert_eq!(record.bssid, bssid(OTHER));
    assert_eq!((other.beacons, other.probe_responses), (0, 1));
    assert!(survey.bss(2).is_none());
    assert_eq!(survey.entries().count(), 2);
    assert_eq!(survey.table().summary().records, 2);
    assert_eq!(survey.counters().frames, 4);
}

#[test]
fn a_repeated_tsf_is_a_duplicate_and_feeds_no_gap() {
    let mut survey = Survey::<2>::new(bssid(STAND));
    let first = beacon(STAND, 1_000);
    take(&mut survey, &first, 0);
    // The same beacon again, much later: no gap, no missed beacons.
    take(&mut survey, &first, 900_000);
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.beacons, 2);
    assert_eq!(tally.duplicates, 1);
    assert_eq!(tally.max_beacon_gap_us, 0);
    assert_eq!(tally.late_gaps, 0);
    assert_eq!(tally.missed_beacons, 0);
    assert!(survey.gap_log().is_empty());
}

#[test]
fn frames_without_a_signal_level_are_counted_outside_its_range() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let now = at(1);
    for (tsf, rssi) in [
        (0, RxEvidence::HardwareObserved(-70)),
        (BI, RxEvidence::Unavailable),
        (2 * BI, RxEvidence::ProtocolValidated(-35)),
        (3 * BI, RxEvidence::Unavailable),
    ] {
        survey.take(now, &beacon(STAND, tsf), &meta(rssi), None, now);
    }
    let (record, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.rssi_dbm, Some(MinMax { min: -70, max: -35 }));
    assert_eq!(tally.without_rssi, 2);
    // The scan record itself keeps the station's marker for the last one.
    assert_eq!(record.rssi, i8::MIN);
}

#[test]
fn a_gap_over_one_and_a_half_intervals_is_late_and_logged_for_the_stand() {
    let mut survey = Survey::<2>::new(bssid(STAND));
    for bss in [STAND, OTHER] {
        take(&mut survey, &beacon(bss, 0), 0);
        // 1.5 intervals exactly: not late; one beacon is missing by TSF
        // (round(1.5) − 1).
        take(&mut survey, &beacon(bss, 3 * BI / 2), 153_600);
        // Two intervals: late, one beacon missing.
        take(&mut survey, &beacon(bss, 3 * BI / 2 + 2 * BI), 358_400);
    }
    for index in 0..2 {
        let (_, tally) = survey.bss(index).unwrap();
        assert_eq!(tally.late_gaps, 1);
        assert_eq!(tally.max_beacon_gap_us, 204_800);
        assert_eq!(tally.missed_beacons, 2);
    }
    assert_eq!(
        survey.gap_log(),
        &[Gap {
            stamp: 358_400,
            gap_us: 204_800,
            missed_by_tsf: Some(1),
        }]
    );
    assert_eq!(survey.gap_log_dropped(), 0);
}

#[test]
fn the_interval_is_the_one_the_later_beacon_advertises() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    take(&mut survey, &frame(8, STAND, 0, 100, None), 0);
    // 204.8 ms is late for 100 TU, not for 200 TU.
    take(&mut survey, &frame(8, STAND, 2 * BI, 200, None), 204_800);
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.late_gaps, 0);
    assert_eq!(tally.missed_beacons, 0);
    assert_eq!(tally.max_beacon_gap_us, 204_800);
}

#[test]
fn a_zero_beacon_interval_takes_no_gap_statistics() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    take(&mut survey, &frame(8, STAND, 0, 0, None), 0);
    take(&mut survey, &frame(8, STAND, 10 * BI, 0, None), 1_024_000);
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.zero_interval, 2);
    assert_eq!(tally.late_gaps, 0);
    assert_eq!(tally.missed_beacons, 0);
    assert_eq!(tally.max_beacon_gap_us, 0);
    assert!(survey.gap_log().is_empty());
}

#[test]
fn a_tsf_that_runs_backwards_counts_no_missed_beacons() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    take(&mut survey, &beacon(STAND, 50 * BI), 0);
    take(&mut survey, &beacon(STAND, 0), 307_200);
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.tsf_regressions, 1);
    assert_eq!(tally.missed_beacons, 0);
    assert_eq!(tally.late_gaps, 1);
    assert_eq!(survey.gap_log()[0].missed_by_tsf, None);
}

#[test]
fn stamps_wrap_and_frames_without_one_end_no_gap() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let start = u32::MAX - 1_000;
    take(&mut survey, &beacon(STAND, 0), start);
    take(&mut survey, &beacon(STAND, BI), start.wrapping_add(102_400));
    let now = at(1);
    // An unstamped beacon two intervals later: its gap is not measured.
    survey.take(now, &beacon(STAND, 3 * BI), &heard(-40), None, now);
    // Nor is the next one's, which follows the unstamped beacon.
    take(
        &mut survey,
        &beacon(STAND, 4 * BI),
        start.wrapping_add(409_600),
    );
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.first_stamp, Some(start));
    assert_eq!(tally.last_stamp, Some(start.wrapping_add(409_600)));
    assert_eq!(tally.stamp_span_us(), Some(409_600));
    assert_eq!(tally.max_beacon_gap_us, 102_400);
    assert_eq!(tally.late_gaps, 0);
    assert_eq!(tally.without_stamp, 1);
    // The TSF still counts the beacon missing before the unstamped one.
    assert_eq!(tally.missed_beacons, 1);
}

#[test]
fn the_handoff_lag_is_the_handoff_less_the_stamp() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let meta = heard(-40);
    for (tsf, stamp, handoff) in [
        (0, 1_000, 1_030),
        (BI, 103_400, 103_520),
        (2 * BI, 205_800, 205_840),
    ] {
        survey.take(
            at(handoff),
            &beacon(STAND, tsf),
            &meta,
            Some(stamp),
            at(handoff),
        );
    }
    // A handoff past the 32-bit wrap of the stamp's clock.
    survey.take(
        at(1),
        &beacon(STAND, 3 * BI),
        &meta,
        Some(u32::MAX - 9),
        at(1 << 32 | 60),
    );
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.handoff_lag_us, Some(MinMax { min: 30, max: 120 }));
}

#[test]
fn the_stamp_and_handoff_spans_cover_the_same_stamped_frames() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let meta = heard(-40);
    // Without a stamped frame there is no span.
    survey.take(at(1_000), &beacon(STAND, 0), &meta, None, at(1_000));
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(
        (tally.stamp_span_us(), tally.handoff_span_us()),
        (None, None)
    );
    // The stamp clock runs at half the handoff clock's rate.
    survey.take(at(5_000), &beacon(STAND, BI), &meta, Some(1_000), at(5_000));
    survey.take(at(6_000), &beacon(STAND, 2 * BI), &meta, None, at(6_000));
    survey.take(
        at(205_000),
        &beacon(STAND, 3 * BI),
        &meta,
        Some(101_000),
        at(205_000),
    );
    // An unstamped frame after the last stamped one moves neither span.
    survey.take(
        at(300_000),
        &beacon(STAND, 4 * BI),
        &meta,
        None,
        at(300_000),
    );
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(
        (tally.first_handoff, tally.last_handoff),
        (Some(5_000), Some(205_000))
    );
    assert_eq!(tally.stamp_span_us(), Some(100_000));
    assert_eq!(tally.handoff_span_us(), Some(200_000));

    // The handoff is kept as its low 32 bits, and its span wraps.
    let mut survey = Survey::<1>::new(bssid(STAND));
    survey.take(
        at(1),
        &beacon(STAND, 0),
        &meta,
        Some(7),
        at(u64::from(u32::MAX) - 9),
    );
    survey.take(at(1), &beacon(STAND, BI), &meta, Some(27), at(1 << 32 | 10));
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.first_handoff, Some(u32::MAX - 9));
    assert_eq!(tally.handoff_span_us(), Some(20));
}

#[test]
fn counters_sort_every_frame_by_what_became_of_it() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let now = at(1);
    let meta = heard(-40);
    // A control frame (ACK) and a truncated header.
    survey.take(now, &[0xd4, 0, 0, 0, 2, 0, 0, 0, 0, 1], &meta, None, now);
    survey.take(now, &beacon(STAND, 0)[..20], &meta, None, now);
    // A QoS data frame.
    let mut data = std::vec![0x88, 0x02, 0, 0];
    data.extend_from_slice(&[[4; 6], STAND, STAND].concat());
    data.extend_from_slice(&[0x10, 0, 0, 0, 0xaa, 0xaa, 3, 0, 0, 0, 8, 0]);
    survey.take(now, &data, &meta, None, now);
    // An authentication frame and a beacon too short for its fixed fields.
    survey.take(now, &frame(11, STAND, 0, 100, None), &meta, None, now);
    survey.take(now, &beacon(STAND, 0)[..30], &meta, None, now);
    // A beacon on channel 11, received while configured to 6.
    survey.take(now, &frame(8, STAND, 0, 100, Some(11)), &meta, None, now);
    survey.take(now, &frame(8, STAND, BI, 100, Some(6)), &meta, None, now);
    // A second BSSID the one-entry table refuses.
    survey.take(now, &beacon(OTHER, 0), &meta, None, now);
    let counters = survey.counters();
    assert_eq!(counters.frames, 8);
    assert_eq!(counters.header_refusals, 2);
    assert_eq!(counters.data, 1);
    assert_eq!(counters.ignored_management, 2);
    assert_eq!(counters.off_channel, 1);
    assert_eq!(counters.table_full, 1);
    assert_eq!(survey.table().summary().records, 1);
}

#[test]
fn the_gap_log_is_bounded_and_counts_what_it_drops() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let late = GAP_LOG_CAPACITY + 3;
    for n in 0..=late as u64 {
        // Every beacon three intervals after the last.
        take(
            &mut survey,
            &beacon(STAND, 3 * BI * n),
            (307_200 * n) as u32,
        );
    }
    assert_eq!(survey.gap_log().len(), GAP_LOG_CAPACITY);
    assert_eq!(survey.gap_log_dropped(), 3);
    assert_eq!(survey.gap_log()[0].stamp, 307_200);
    let (_, tally) = survey.bss(0).unwrap();
    assert_eq!(tally.late_gaps, late as u32);
    assert_eq!(tally.missed_beacons, 2 * late as u32);
}

#[test]
fn missed_beacons_round_the_tsf_periods() {
    let interval = NonZeroU64::new(BI).unwrap();
    assert_eq!(missed_beacons(0, interval), 0);
    assert_eq!(missed_beacons(BI, interval), 0);
    assert_eq!(missed_beacons(BI * 14 / 10, interval), 0);
    assert_eq!(missed_beacons(BI * 15 / 10, interval), 1);
    assert_eq!(missed_beacons(BI * 26 / 10, interval), 2);
    assert_eq!(missed_beacons(u64::MAX, interval), u32::MAX);
    assert_eq!(
        missed_beacons(u64::MAX, NonZeroU64::new(u64::MAX).unwrap()),
        0
    );
}

fn survey_never_panics(count: usize) {
    use oer_ieee80211_mac::fuzz::{XorShift, inputs};
    let mut htc = beacon(STAND, BI);
    htc[1] |= 0x80;
    htc.splice(24..24, [1, 2, 3, 4]);
    let seeds = [
        beacon(STAND, 0),
        frame(5, OTHER, 9, 100, Some(6)),
        frame(8, OTHER, 0, 0, Some(11)),
        htc,
    ];
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let mut survey = Survey::<3>::new(bssid(STAND));
    let mut rng = XorShift::new(7);
    inputs(&seeds, count, 96, |frame| {
        let value = rng.next();
        let rssi = match value % 3 {
            0 => RxEvidence::Unavailable,
            _ => RxEvidence::HardwareObserved(value as i8),
        };
        let stamp = (value & 4 != 0).then_some((value >> 8) as u32);
        survey.take(
            at(value >> 40),
            frame,
            &meta(rssi),
            stamp,
            Instant::from_micros(value >> 1),
        );
    });
    for (_, tally) in survey.entries() {
        let _ = tally.stamp_span_us();
        let _ = tally.handoff_span_us();
    }
    assert!(survey.counters().frames > 0);
}

#[test]
fn the_survey_never_panics_on_received_bytes() {
    survey_never_panics(oer_ieee80211_mac::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn the_survey_never_panics_on_received_bytes_full() {
    survey_never_panics(oer_ieee80211_mac::fuzz::FULL);
}

#[test]
fn a_beacon_of_another_protocol_version_is_a_header_refusal_not_a_record() {
    let mut survey = Survey::<1>::new(bssid(STAND));
    let mut versioned = beacon(STAND, 0);
    // Protocol version 1 in the low two bits of the frame control field.
    versioned[0] |= 0b01;
    survey.take(at(0), &versioned, &heard(-40), None, at(0));
    let counters = survey.counters();
    assert_eq!(counters.frames, 1);
    assert_eq!(counters.header_refusals, 1);
    assert!(survey.bss(0).is_none());
    // The same beacon at version 0 is recorded.
    survey.take(at(0), &beacon(STAND, 0), &heard(-40), None, at(0));
    assert!(survey.bss(0).is_some());
}
