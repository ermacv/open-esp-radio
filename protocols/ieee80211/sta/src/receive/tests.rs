use super::{Management, Received, classify, observe_scan};
use oer_ieee80211_lower_mac::{Channel, ChannelWidth, RxEvidence, RxMeta};
use oer_ieee80211_mac::fuzz::{FULL, QUICK, inputs};
use oer_ieee80211_mac::receive::RxDropReason;
use oer_ieee80211_mac::scan::{ScanObservation, ScanTable};
use oer_time::Instant;
use std::vec::Vec;

const BSSID: [u8; 6] = [2, 2, 3, 4, 5, 6];

/// A beacon (subtype 8) or probe response (subtype 5) from [`BSSID`] with
/// the SSID `test`, a 100 TU interval and `elements` after the SSID.
fn management(subtype: u8, elements: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 36];
    frame[0] = subtype << 4;
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..32].copy_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34] = 0x01;
    frame.extend_from_slice(&[0, 4, b't', b'e', b's', b't']);
    frame.extend_from_slice(elements);
    frame
}

/// A QoS data frame from the access point with an LLC/SNAP body.
fn data() -> Vec<u8> {
    let mut frame = std::vec![0x88, 0x02, 0, 0];
    frame.extend_from_slice(&[4; 6]);
    frame.extend_from_slice(&BSSID);
    frame.extend_from_slice(&BSSID);
    frame.extend_from_slice(&[0x10, 0, 0, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2]);
    frame
}

fn on_channel(number: u8, rssi: RxEvidence<i8>) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap());
    meta.rssi_dbm = rssi;
    meta
}

fn classified_management(frame: &[u8]) -> Management<'_> {
    match classify(frame) {
        Ok(Received::Management(management)) => management,
        other => panic!("not a management frame: {other:?}"),
    }
}

#[test]
fn classify_sorts_management_from_data() {
    let beacon = management(8, &[]);
    let sorted = classified_management(&beacon);
    assert_eq!(sorted.subtype(), 8);
    assert_eq!(sorted.frame(), beacon.as_slice());
    assert_eq!(sorted.header().address3(), BSSID);
    let probe = management(5, &[]);
    assert_eq!(classified_management(&probe).subtype(), 5);

    let data = data();
    match classify(&data) {
        Ok(Received::Data(header)) => {
            assert!(header.control().is_data());
            assert_eq!(header.frame(), data.as_slice());
        }
        other => panic!("not a data frame: {other:?}"),
    }
}

#[test]
fn classify_refuses_control_and_short_frames() {
    // An ACK: a control frame, which has no data or management header.
    let ack = [0xd4, 0, 0, 0, 2, 2, 3, 4, 5, 6];
    assert!(matches!(classify(&ack), Err(RxDropReason::Header)));
    // An RTS padded to a management header's length is still control.
    let mut rts = std::vec![0_u8; 36];
    rts[0] = 0xb4;
    assert!(matches!(classify(&rts), Err(RxDropReason::Header)));
    // A beacon cut inside its sequence control field.
    let beacon = management(8, &[]);
    assert!(matches!(classify(&beacon[..23]), Err(RxDropReason::Header)));
    assert!(matches!(classify(&[]), Err(RxDropReason::Header)));
}

#[test]
fn observe_scan_takes_the_configured_channel_without_a_ds_element() {
    let mut scan = ScanTable::<2>::new();
    let beacon = management(8, &[]);
    let now = Instant::from_micros(7_000);
    let meta = on_channel(6, RxEvidence::HardwareObserved(-40));
    assert_eq!(
        observe_scan(&mut scan, now, classified_management(&beacon), &meta),
        ScanObservation::Inserted { index: 0 }
    );
    let record = scan.records().next().unwrap();
    assert_eq!(record.channel, 6);
    assert_eq!(record.rssi, -40);
    assert_eq!(record.received_at, Some(now));
    assert_eq!(record.ssid_bytes(), b"test");

    // With a DS Parameter Set the element names the channel.
    let mut scan = ScanTable::<2>::new();
    let beacon = management(8, &[3, 1, 11]);
    let _ = observe_scan(&mut scan, now, classified_management(&beacon), &meta);
    assert_eq!(scan.records().next().unwrap().channel, 11);
}

#[test]
fn observe_scan_records_an_unavailable_signal_as_the_lowest_level() {
    let beacon = management(8, &[]);
    let now = Instant::from_micros(1);
    for (rssi, recorded) in [
        (RxEvidence::Unavailable, i8::MIN),
        (RxEvidence::HardwareObserved(-55), -55),
        (RxEvidence::ProtocolValidated(-61), -61),
    ] {
        let mut scan = ScanTable::<1>::new();
        let _ = observe_scan(
            &mut scan,
            now,
            classified_management(&beacon),
            &on_channel(1, rssi),
        );
        assert_eq!(scan.records().next().unwrap().rssi, recorded);
    }
}

#[test]
fn observe_scan_ignores_management_that_is_no_beacon_or_probe_response() {
    // An authentication frame (subtype 11).
    let authentication = management(11, &[]);
    let mut scan = ScanTable::<1>::new();
    assert_eq!(
        observe_scan(
            &mut scan,
            Instant::from_micros(1),
            classified_management(&authentication),
            &on_channel(1, RxEvidence::HardwareObserved(-40)),
        ),
        ScanObservation::Ignored
    );
    assert_eq!(scan.summary().records, 0);
}

fn classify_and_observe_never_panic(count: usize) {
    let mut htc = management(8, &[3, 1, 6, 48, 2, 1, 0]);
    htc[1] |= 0x80;
    htc.splice(24..24, [1, 2, 3, 4]);
    let seeds = [management(8, &[3, 1, 6]), management(5, &[]), htc, data()];
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let mut scan = ScanTable::<4>::new();
    let mut step = 0_u8;
    inputs(&seeds, count, 128, |frame| {
        step = step.wrapping_add(1);
        let rssi = match step % 3 {
            0 => RxEvidence::Unavailable,
            1 => RxEvidence::HardwareObserved(step as i8),
            _ => RxEvidence::ProtocolValidated(step as i8),
        };
        let meta = on_channel(1 + step % 13, rssi);
        match classify(frame) {
            Ok(Received::Management(management)) => {
                let _ = (management.subtype(), management.header().body());
                assert_eq!(management.frame(), frame);
                let _ = observe_scan(&mut scan, Instant::from_micros(1), management, &meta);
            }
            Ok(Received::Data(header)) => assert!(header.control().is_data()),
            Err(reason) => assert_eq!(reason, RxDropReason::Header),
        }
        if step == 0 {
            scan.clear();
        }
    });
}

#[test]
fn classify_and_observe_scan_never_panic_on_received_bytes() {
    classify_and_observe_never_panic(QUICK);
}

#[test]
#[ignore = "the full run"]
fn classify_and_observe_scan_never_panic_on_received_bytes_full() {
    classify_and_observe_never_panic(FULL);
}
