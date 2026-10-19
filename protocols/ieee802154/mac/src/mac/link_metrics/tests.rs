use super::*;

const EXTENDED: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

fn lqi_and_margin() -> LinkMetrics {
    LinkMetrics {
        lqi: true,
        link_margin: true,
        ..LinkMetrics::NONE
    }
}

/// An initiator is matched by either address and its data follows the
/// fixed order and scaling of `GetEnhAckData`.
#[test]
fn an_initiator_gets_its_metrics_in_the_fixed_order() {
    let mut probing = EnhAckProbing::<2>::new(-97);
    probing
        .configure(0x1234, EXTENDED, lqi_and_margin())
        .unwrap();
    let data = probing.data(FrameAddress::Short([0x34, 0x12]), 200, -40);
    // Margin 57 dB scales to 57 * 255 / 130.
    assert_eq!(data.bytes(), [200, 111]);
    assert_eq!(
        probing.data(FrameAddress::Extended(EXTENDED), 200, -40),
        data
    );
    assert!(
        probing
            .data(FrameAddress::Short([0, 0]), 200, -40)
            .bytes()
            .is_empty()
    );
}

/// RSSI is scaled from [-130, 0], and only when fewer than two bytes were
/// written.
#[test]
fn rssi_fits_only_behind_one_other_metric() {
    let mut probing = EnhAckProbing::<1>::new(-97);
    let rssi = LinkMetrics {
        rssi: true,
        ..LinkMetrics::NONE
    };
    probing.configure(1, EXTENDED, rssi).unwrap();
    assert_eq!(
        probing.data(FrameAddress::Short([1, 0]), 0, -65).bytes(),
        [127]
    );
    let all = LinkMetrics {
        pdu_count: true,
        lqi: true,
        link_margin: true,
        rssi: true,
    };
    probing.configure(1, EXTENDED, all).unwrap();
    assert_eq!(probing.data(FrameAddress::Short([1, 0]), 9, -65).len, 2);
}

/// The link margin is zero below the noise floor and for an invalid RSSI.
#[test]
fn the_link_margin_is_clamped_at_zero() {
    assert_eq!(link_margin(-97, -40), 57);
    assert_eq!(link_margin(-97, -100), 0);
    assert_eq!(link_margin(-97, 127), 0);
    assert_eq!(link_margin(0, -40), 0);
}

/// Configuring replaces an initiator, clearing removes it, and a full
/// table refuses another one.
#[test]
fn the_table_replaces_removes_and_refuses() {
    let mut probing = EnhAckProbing::<1>::new(-97);
    let lqi = LinkMetrics {
        lqi: true,
        ..LinkMetrics::NONE
    };
    probing.configure(1, EXTENDED, lqi).unwrap();
    probing.configure(1, EXTENDED, lqi_and_margin()).unwrap();
    assert_eq!(
        probing.metrics(FrameAddress::Short([1, 0])),
        Some(lqi_and_margin())
    );
    assert_eq!(probing.configure(2, [9; 8], lqi), Err(ProbingError::NoBufs));
    probing.configure(1, EXTENDED, LinkMetrics::NONE).unwrap();
    assert_eq!(
        probing.configure(1, EXTENDED, LinkMetrics::NONE),
        Err(ProbingError::NotFound)
    );
    probing.configure(2, [9; 8], lqi).unwrap();
    probing.reset();
    assert_eq!(probing.metrics(FrameAddress::Short([2, 0])), None);
}

/// The most recently added initiator matches an extended address first.
#[test]
fn the_newest_initiator_matches_first() {
    let mut probing = EnhAckProbing::<2>::new(-97);
    let lqi = LinkMetrics {
        lqi: true,
        ..LinkMetrics::NONE
    };
    probing.configure(1, EXTENDED, lqi).unwrap();
    probing.configure(2, EXTENDED, lqi_and_margin()).unwrap();
    assert_eq!(
        probing.metrics(FrameAddress::Extended(EXTENDED)),
        Some(lqi_and_margin())
    );
}

/// The IE is a vendor-specific header IE with the Thread OUI and the
/// probing subtype, as `otMacFrameGenerateEnhAckProbingIe` writes it.
#[test]
fn the_probing_ie_carries_the_thread_oui_and_the_data() {
    let mut probing = EnhAckProbing::<1>::new(-97);
    probing.configure(1, EXTENDED, lqi_and_margin()).unwrap();
    let data = probing.data(FrameAddress::Short([1, 0]), 200, -40);
    let mut ie = [0; ENH_ACK_PROBING_IE_CAPACITY];
    assert_eq!(data.write_ie(&mut ie), 8);
    assert_eq!(ie, [0x06, 0x00, 0x9b, 0xb8, 0xea, 0x00, 200, 111]);
    assert_eq!(data.write_ie(&mut ie[..7]), 0);
    assert_eq!(ProbingData::default().write_ie(&mut ie), 0);
}

/// A replaced table holds exactly the new initiators, the first listed
/// matching first, and keeps the noise floor.
#[test]
fn replacing_the_initiators_keeps_the_newest_first() {
    let lqi = LinkMetrics {
        lqi: true,
        ..LinkMetrics::NONE
    };
    let rssi = LinkMetrics {
        rssi: true,
        ..LinkMetrics::NONE
    };
    let extended = [8, 7, 6, 5, 4, 3, 2, 1];
    let mut probing = EnhAckProbing::<4>::new(-97);
    // Short 2 was added after short 1, for the same device.
    probing.replace(&[
        ProbingInitiator {
            short_address: 2,
            extended_address: extended,
            metrics: rssi,
        },
        ProbingInitiator {
            short_address: 1,
            extended_address: extended,
            metrics: lqi,
        },
    ]);
    assert_eq!(
        probing.metrics(FrameAddress::Extended(extended)),
        Some(rssi)
    );
    assert_eq!(probing.metrics(FrameAddress::Short([1, 0])), Some(lqi));

    probing.replace(&[ProbingInitiator {
        short_address: 1,
        extended_address: extended,
        metrics: lqi,
    }]);
    assert_eq!(probing.metrics(FrameAddress::Short([2, 0])), None);
    assert_eq!(probing.metrics(FrameAddress::Extended(extended)), Some(lqi));
    assert_eq!(probing.noise_floor(), -97);
}
