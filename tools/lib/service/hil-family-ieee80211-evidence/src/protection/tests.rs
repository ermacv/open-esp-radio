use super::*;
use crate::air::test_support::frame;

const AP: MacAddress = MacAddress([0xbe, 0xfc, 0xe7, 0xae, 0xbb, 0xd3]);
const TARGET: MacAddress = MacAddress([0x02, 0, 0, 0, 0, 0x10]);
const LAPTOP: MacAddress = MacAddress([0x02, 0, 0, 0, 0, 0x20]);

fn at(
    time: u64,
    kind: FrameKind,
    transmitter: Option<MacAddress>,
    receiver: MacAddress,
) -> AirFrame {
    let mut record = frame(time, kind);
    record.mac_time_micros = Some(time);
    record.transmitter = transmitter;
    record.receiver = Some(receiver);
    record
}

/// A BSS with the OFDM basic set 6/12/24 Mb/s.
fn bss(erp_use_protection: bool) -> BssRates {
    BssRates {
        basic_kbps: [6_000, 12_000, 24_000].into(),
        erp_use_protection,
    }
}

fn control(mut record: AirFrame, phy: AirPhy, rate_kbps: u32) -> AirFrame {
    record.phy = Some(phy);
    record.rate_kbps = Some(rate_kbps);
    record.short_preamble = Some(false);
    record
}

/// One exchange starting at `start`: optional RTS/CTS at `phy`/`rate`, two
/// A-MPDU subframes and a BlockAck, with the RTS carrying `duration`.
fn exchange(
    start: u64,
    protected: bool,
    phy: AirPhy,
    rate_kbps: u32,
    duration: u16,
) -> Vec<AirFrame> {
    let mut frames = Vec::new();
    let mut time = start;
    if protected {
        let mut rts = control(at(time, FrameKind::RTS, Some(TARGET), AP), phy, rate_kbps);
        rts.duration_micros = Some(duration);
        let rts_airtime = control_airtime(&rts, RTS_BYTES).unwrap();
        frames.push(rts);
        time += rts_airtime + 10;
        let mut cts = control(at(time, FrameKind::CTS, None, TARGET), phy, rate_kbps);
        cts.duration_micros =
            Some(duration.saturating_sub(u16::try_from(rts_airtime + 10).unwrap()));
        frames.push(cts);
        time += control_airtime(&frames[1], ACK_BYTES).unwrap() + 10;
    }
    // The observer stamps TSFT on one MPDU of the A-MPDU.
    let mut subframe = at(time, FrameKind(0x28), Some(TARGET), AP);
    subframe.mac_time_micros = None;
    subframe.ampdu_reference = Some(u32::try_from(start).unwrap());
    frames.push(subframe.clone());
    subframe.mac_time_micros = Some(time);
    frames.push(subframe);
    time += 300;
    frames.push(control(
        at(time, FrameKind::BLOCK_ACK, Some(AP), TARGET),
        AirPhy::Ofdm,
        24_000,
    ));
    frames
}

#[test]
fn every_protected_ppdu_with_covering_nav_passes() {
    let frames = (0..10)
        .flat_map(|index| exchange(index * 10_000, true, AirPhy::Ofdm, 24_000, 400))
        .collect::<Vec<_>>();
    let evidence = Flow::station(&frames, AP, None)
        .and_then(|flow| analyze(&frames, flow, &bss(false)))
        .unwrap();
    assert_eq!(evidence.target.as_deref(), Some("02:00:00:00:00:10"));
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (10, 10));
    assert_eq!(evidence.protected_basis_points(), 10_000);
    assert_eq!(evidence.target_rts, 10);
    assert_eq!(evidence.wrong_control_rate, 0);
    assert_eq!((evidence.nav_evaluated, evidence.nav_short), (10, 0));
}

#[test]
fn unprotected_ppdus_wrong_rates_and_short_nav_are_counted() {
    let mut frames = exchange(0, true, AirPhy::Ofdm, 24_000, 400);
    frames.extend(exchange(10_000, false, AirPhy::Ofdm, 24_000, 0));
    frames.extend(exchange(20_000, true, AirPhy::Ofdm, 24_000, 100));
    let evidence = Flow::station(&frames, AP, None)
        .and_then(|flow| analyze(&frames, flow, &bss(true)))
        .unwrap();
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (3, 2));
    assert_eq!(evidence.protected_basis_points(), 6_666);
    assert_eq!(evidence.wrong_control_rate, 2);
    assert_eq!(evidence.nav_short, 1);
    assert!(evidence.first_nav_shortfall_micros.unwrap() > 0);
    let dsss = exchange(0, true, AirPhy::HrDsss, 11_000, 600);
    let evidence = Flow::station(&dsss, AP, None)
        .and_then(|flow| analyze(&dsss, flow, &bss(true)))
        .unwrap();
    assert_eq!((evidence.wrong_control_rate, evidence.nav_short), (0, 0));
}

#[test]
fn either_observed_half_of_an_exchange_shows_protection() {
    let mut lost_cts = exchange(0, true, AirPhy::Ofdm, 24_000, 600);
    lost_cts.remove(1);
    let mut lost_rts = exchange(10_000, true, AirPhy::Ofdm, 24_000, 600);
    lost_rts.remove(0);
    let frames = [lost_cts, lost_rts].concat();
    let evidence = Flow::station(&frames, AP, None)
        .and_then(|flow| analyze(&frames, flow, &bss(false)))
        .unwrap();
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (2, 2));
    assert_eq!((evidence.cts_unobserved, evidence.rts_unobserved), (1, 1));
    // Only the exchange whose RTS was observed has a judged NAV.
    assert_eq!(evidence.nav_evaluated, 1);
}

#[test]
fn the_target_is_the_dominant_sender_other_than_the_peer() {
    let mut frames = exchange(0, true, AirPhy::Ofdm, 24_000, 400);
    for index in 0..5 {
        frames.push(at(50_000 + index, FrameKind(0x28), Some(LAPTOP), AP));
    }
    assert!(
        Flow::station(&frames, AP, None)
            .and_then(|flow| analyze(&frames, flow, &bss(false)))
            .is_err()
    );
    let evidence = Flow::station(&frames, AP, Some(LAPTOP))
        .and_then(|flow| analyze(&frames, flow, &bss(false)))
        .unwrap();
    assert_eq!(evidence.target.as_deref(), Some("02:00:00:00:00:10"));
    assert!(Flow::station(&[], AP, None).is_err());
    let mut untimed = frames.clone();
    untimed[1].mac_time_micros = None;
    assert!(
        Flow::station(&untimed, AP, Some(LAPTOP))
            .and_then(|flow| analyze(&untimed, flow, &bss(false)))
            .is_err()
    );
}

#[test]
fn control_airtime_follows_the_phy() {
    let mut rts = control(frame(0, FrameKind::RTS), AirPhy::Dsss, 1_000);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(192 + 160));
    rts.phy = Some(AirPhy::HrDsss);
    rts.rate_kbps = Some(11_000);
    rts.short_preamble = Some(true);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(96 + 15));
    rts.phy = Some(AirPhy::Ofdm);
    rts.rate_kbps = Some(24_000);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(20 + 2 * 4 + 6));
}

#[test]
fn an_access_point_flow_is_the_ap_data_to_its_other_station() {
    // The target AP serves the laptop at legacy rates and protects its HT
    // PPDUs to the second station.
    const HT_CLIENT: MacAddress = MacAddress([0x02, 0, 0, 0, 0, 0x30]);
    let retarget = |frame: &mut AirFrame| {
        for address in [&mut frame.transmitter, &mut frame.receiver] {
            *address = match *address {
                Some(TARGET) => Some(AP),
                Some(AP) => Some(HT_CLIENT),
                other => other,
            };
        }
    };
    let mut frames = (0..3)
        .flat_map(|index| exchange(index * 10_000, true, AirPhy::Ofdm, 24_000, 600))
        .collect::<Vec<_>>();
    frames.iter_mut().for_each(retarget);
    for index in 0..5 {
        frames.push(at(40_000 + index, FrameKind(0x28), Some(AP), LAPTOP));
    }
    let flow = Flow::access_point(&frames, LAPTOP).unwrap();
    assert_eq!(
        flow,
        Flow {
            target: AP,
            receiver: HT_CLIENT
        }
    );
    let evidence = analyze(&frames, flow, &bss(false)).unwrap();
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (3, 3));
    assert_eq!(evidence.nav_short, 0);
}

#[test]
fn control_rates_follow_the_basic_set_and_erp() {
    let rts = |phy, rate| control(frame(0, FrameKind::RTS), phy, rate);
    let plain = bss(false);
    assert!(control_rate_allowed(&rts(AirPhy::Ofdm, 24_000), &plain));
    assert!(control_rate_allowed(&rts(AirPhy::HrDsss, 11_000), &plain));
    // 36 Mb/s is neither basic nor mandatory here.
    assert!(!control_rate_allowed(&rts(AirPhy::Ofdm, 36_000), &plain));
    let mut basic_36 = bss(false);
    basic_36.basic_kbps.insert(36_000);
    assert!(control_rate_allowed(&rts(AirPhy::Ofdm, 36_000), &basic_36));
    // ERP protection requires DSSS/HR even for a basic OFDM rate.
    assert!(!control_rate_allowed(
        &rts(AirPhy::Ofdm, 24_000),
        &bss(true)
    ));
    assert!(control_rate_allowed(
        &rts(AirPhy::HrDsss, 11_000),
        &bss(true)
    ));
    assert!(!control_rate_allowed(&rts(AirPhy::Ht, 65_000), &plain));
    assert!(!control_rate_allowed(&frame(0, FrameKind::RTS), &plain));
}

#[test]
fn an_ack_after_an_a_mpdu_answers_another_exchange() {
    // The observer lost the BlockAck; the target's next Ack answers a single
    // MPDU to another station and must not judge this NAV.
    let mut frames = exchange(0, true, AirPhy::Ofdm, 24_000, 600);
    let block_ack = frames.pop().unwrap();
    frames.push(control(
        at(
            block_ack.mac_time_micros.unwrap() + 800,
            FrameKind::ACK,
            None,
            TARGET,
        ),
        AirPhy::Ofdm,
        24_000,
    ));
    let evidence = Flow::station(&frames, AP, None)
        .and_then(|flow| analyze(&frames, flow, &bss(false)))
        .unwrap();
    assert_eq!((evidence.protected_ppdus, evidence.nav_evaluated), (1, 0));
    assert_eq!(evidence.nav_short, 0);
}
