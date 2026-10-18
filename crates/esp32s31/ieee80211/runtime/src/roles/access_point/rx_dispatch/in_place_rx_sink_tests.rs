
use oer_ieee80211_softmac::MacRxMetadata;

use super::*;

#[test]
fn active_tx_protocol_consumer_has_no_hardware_capability() {
    assert!(rx_protocol_consumer_has_hardware(false));
    assert!(!rx_protocol_consumer_has_hardware(true));
}

fn event<'a>(payload: &'a [u8], amsdu: bool) -> ApRxEvent<'a> {
    ApRxEvent {
        frame: EthernetFrameParts {
            destination: [1, 2, 3, 4, 5, 6],
            source: [7, 8, 9, 10, 11, 12],
            ether_type: 0x0800,
            payload,
        },
        raw: payload,
        amsdu,
        metadata: MacRxMetadata::unavailable(),
    }
}

#[test]
fn captures_one_ordinary_frame_as_staging_offsets() {
    let raw = [0_u8; 64];
    let mut sink = InPlaceAccessPointRxSink::new(&raw);

    sink.publish(event(&raw[17..43], false));

    let publication = sink.publication.expect("ordinary frame is captured");
    assert_eq!(publication.payload_offset, 17);
    assert_eq!(publication.payload_length, 26);
    assert!(!sink.unsupported);
}

#[test]
fn rejects_amsdu_and_payloads_outside_the_staging_owner() {
    let raw = [0_u8; 64];
    let external = [0_u8; 8];

    let mut amsdu = InPlaceAccessPointRxSink::new(&raw);
    amsdu.publish(event(&raw[16..24], true));
    assert!(amsdu.publication.is_none());
    assert!(amsdu.unsupported);

    let mut outside = InPlaceAccessPointRxSink::new(&raw);
    outside.publish(event(&external, false));
    assert!(outside.publication.is_none());
    assert!(outside.unsupported);
}

#[test]
fn current_frame_joins_an_older_deferred_reorder_release() {
    assert!(can_publish_ap_rx_in_place(true, false, 0));
    assert!(!can_publish_ap_rx_in_place(true, false, 64));
    assert!(!can_publish_ap_rx_in_place(true, true, 0));
    assert!(!can_publish_ap_rx_in_place(false, false, 0));
}
