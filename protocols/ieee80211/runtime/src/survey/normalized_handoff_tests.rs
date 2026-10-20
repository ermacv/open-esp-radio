//! Scan-table behavior over normalized port metadata from the RX handoff.

use super::{Gap, MinMax, Survey, SurveyCounters};
use oer_ieee80211_port::{Channel, ChannelWidth};
use oer_ieee80211_wire::address::{Bssid, IndividualAddress};
use oer_time::Instant;

use oer_ieee80211_port::{RxEvidence, RxMeta};
use oer_time::Clock;
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

const STAND: [u8; 6] = [2, 0, 0, 0, 0, 1];
const OTHER: [u8; 6] = [2, 0, 0, 0, 0, 2];
/// 100 TU in µs.
const BI: u64 = 102_400;
const CHANNEL: u8 = 6;

fn bssid(address: [u8; 6]) -> Bssid {
    Bssid::new(IndividualAddress::new(address).unwrap()).unwrap()
}

/// A beacon (subtype 8) or probe response (subtype 5) from `bss`.
fn management(subtype: u8, bss: [u8; 6], tsf: u64, ssid: &[u8], ds: Option<u8>) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 36];
    frame[0] = subtype << 4;
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&bss);
    frame[16..22].copy_from_slice(&bss);
    frame[24..32].copy_from_slice(&tsf.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34] = 0x01;
    frame.extend_from_slice(&[0, ssid.len() as u8]);
    frame.extend_from_slice(ssid);
    if let Some(channel) = ds {
        frame.extend_from_slice(&[3, 1, channel]);
    }
    frame
}

/// Normalized MPDU, received signal and carried handoff timing.
struct Unit {
    mpdu: Vec<u8>,
    rssi: i8,
    handoff: u64,
    lag: u32,
}

fn unit(mpdu: Vec<u8>, rssi: i8, handoff: u64, lag: u32) -> Unit {
    Unit {
        mpdu,
        rssi,
        handoff,
        lag,
    }
}

#[test]
fn normalized_handoff_frames_reach_the_scan_table() {
    let clock: VirtualClock = VirtualClock::new();
    let mut survey = Survey::<4>::new(bssid(STAND));
    let channel = Channel::ghz2_4(CHANNEL, ChannelWidth::Mhz20).unwrap();
    // One second past the clock's start.
    let base = oer_time_virtual::START.as_micros() + 1_000_000;
    let stand = |tsf| management(8, STAND, tsf, b"stand", Some(CHANNEL));
    let mut data = std::vec![0_u8; 24];
    data[0] = 0x08;
    data[10..16].copy_from_slice(&OTHER);
    let units = [
        unit(stand(0), -40, base, 30),
        unit(
            management(8, OTHER, 5_000, b"", None),
            -70,
            base + 10_000,
            30,
        ),
        unit(stand(BI), -44, base + BI, 45),
        unit(
            management(5, OTHER, 6_000, b"hidden", None),
            -68,
            base + BI + 2_000,
            30,
        ),
        // The same beacon again: a repeated TSF.
        unit(stand(BI), -42, base + BI + 4_000, 30),
        // Two beacons missed: a late gap of the stand BSSID.
        unit(stand(4 * BI), -46, base + 4 * BI, 30),
        unit(data, -50, base + 4 * BI + 1_000, 30),
    ];

    let mut stamps = Vec::new();
    for unit in &units {
        let handoff = Instant::from_micros(unit.handoff);
        let stamp = unit.handoff as u32 - unit.lag;
        stamps.push(stamp);
        clock.advance_to(handoff);
        let mut metadata = RxMeta::unavailable(channel);
        metadata.rssi_dbm = RxEvidence::HardwareObserved(unit.rssi);
        survey.take(clock.now(), &unit.mpdu, &metadata, Some(stamp), handoff);
    }

    assert_eq!(
        survey.counters(),
        SurveyCounters {
            frames: 7,
            header_refusals: 0,
            data: 1,
            ignored_management: 0,
            table_full: 0,
            off_channel: 0,
        }
    );
    let (record, tally) = survey.bss(0).unwrap();
    assert_eq!(record.bssid, bssid(STAND));
    assert_eq!(record.ssid_bytes(), b"stand");
    assert_eq!((record.channel, record.beacon_interval_tu), (CHANNEL, 100));
    assert_eq!(
        (tally.beacons, tally.probe_responses, tally.duplicates),
        (4, 0, 1)
    );
    assert_eq!(tally.rssi_dbm, Some(MinMax { min: -46, max: -40 }));
    assert_eq!(tally.without_rssi, 0);
    assert_eq!(tally.handoff_lag_us, Some(MinMax { min: 30, max: 45 }));
    assert_eq!(
        (tally.first_stamp, tally.last_stamp),
        (Some(stamps[0]), Some(stamps[5]))
    );
    assert_eq!(tally.max_beacon_gap_us, stamps[5] - stamps[2]);
    assert_eq!((tally.late_gaps, tally.missed_beacons), (1, 2));
    assert_eq!(
        survey.gap_log(),
        [Gap {
            stamp: stamps[5],
            gap_us: stamps[5] - stamps[2],
            missed_by_tsf: Some(2),
        }]
    );

    // The hidden-SSID beacon's BSS learned its SSID from the probe
    // response.
    let (record, tally) = survey.bss(1).unwrap();
    assert_eq!(record.bssid, bssid(OTHER));
    assert_eq!(record.ssid_bytes(), b"hidden");
    assert_eq!((tally.beacons, tally.probe_responses), (1, 1));
    assert_eq!(tally.rssi_dbm, Some(MinMax { min: -70, max: -68 }));
    assert!(survey.bss(2).is_none());
}
