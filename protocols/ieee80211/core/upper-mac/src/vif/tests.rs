use super::*;

const CONTROL_FRAME: u16 = 0x0004;
const DATA_FRAME: u16 = 0x0008;
const TO_DS: u16 = 0x0100;
const FROM_DS: u16 = 0x0200;
const PROBE_REQUEST_SUBTYPE: u16 = 4;

const STA: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];
const GROUP: [u8; 6] = [0xff; 6];
const ADDRESSES: StaApRxAddresses = StaApRxAddresses {
    station: STA,
    station_bssid: UPLINK,
    access_point: AP,
};

fn frame(frame_control: u16, receiver: [u8; 6], transmitter: [u8; 6], third: [u8; 6]) -> [u8; 24] {
    let mut frame = [0; 24];
    frame[0..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&transmitter);
    frame[16..22].copy_from_slice(&third);
    frame
}

#[test]
fn routes_from_ds_unicast_and_multicast_to_station() {
    assert_eq!(
        classify_sta_ap_rx(&frame(DATA_FRAME | FROM_DS, STA, UPLINK, PEER), ADDRESSES),
        StaApRxRoute::Interface(StaApVif::Station)
    );
    assert_eq!(
        classify_sta_ap_rx(&frame(DATA_FRAME | FROM_DS, GROUP, UPLINK, PEER), ADDRESSES),
        StaApRxRoute::Interface(StaApVif::Station)
    );
}

#[test]
fn routes_to_ds_data_and_broadcast_probe_request_to_access_point() {
    assert_eq!(
        classify_sta_ap_rx(&frame(DATA_FRAME | TO_DS, AP, PEER, STA), ADDRESSES),
        StaApRxRoute::Interface(StaApVif::AccessPoint)
    );
    assert_eq!(
        classify_sta_ap_rx(
            &frame(PROBE_REQUEST_SUBTYPE << 4, GROUP, PEER, GROUP),
            ADDRESSES,
        ),
        StaApRxRoute::Interface(StaApVif::AccessPoint)
    );
}

#[test]
fn routes_upstream_beacon_by_transmitter_and_bssid() {
    assert_eq!(
        classify_sta_ap_rx(&frame(8 << 4, GROUP, UPLINK, UPLINK), ADDRESSES),
        StaApRxRoute::Interface(StaApVif::Station)
    );
}

#[test]
fn ambiguous_and_unknown_headers_fail_closed() {
    let same_address = StaApRxAddresses {
        access_point: STA,
        ..ADDRESSES
    };
    assert_eq!(
        classify_sta_ap_rx(&frame(DATA_FRAME | TO_DS, STA, PEER, UPLINK), same_address),
        StaApRxRoute::Interface(StaApVif::AccessPoint)
    );
    assert_eq!(
        classify_sta_ap_rx(&frame(CONTROL_FRAME, STA, PEER, UPLINK), same_address),
        StaApRxRoute::Ambiguous
    );
    let mut four_address = [0; 30];
    four_address[..24].copy_from_slice(&frame(DATA_FRAME | TO_DS | FROM_DS, AP, PEER, UPLINK));
    assert_eq!(
        classify_sta_ap_rx(&four_address, ADDRESSES),
        StaApRxRoute::Foreign
    );
    // A four-address frame that ends before Address 4 is malformed.
    assert_eq!(
        classify_sta_ap_rx(&four_address[..24], ADDRESSES),
        StaApRxRoute::Malformed
    );
    assert_eq!(
        classify_sta_ap_rx(&[0; 9], ADDRESSES),
        StaApRxRoute::Malformed
    );
}

fn classify_sta_ap_rx_never_panics(count: usize) {
    let seeds = [
        frame(DATA_FRAME | FROM_DS, STA, UPLINK, PEER),
        frame(DATA_FRAME | TO_DS, AP, PEER, STA),
        frame(PROBE_REQUEST_SUBTYPE << 4, GROUP, PEER, GROUP),
        frame(8 << 4, GROUP, UPLINK, UPLINK),
        frame(CONTROL_FRAME | 0xb0, AP, PEER, PEER),
    ];
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(<[u8; 24]>::as_slice).collect();
    oer_ieee80211_wire::fuzz::inputs(&seeds, count, 40, |mpdu| {
        let _ = classify_sta_ap_rx(mpdu, ADDRESSES);
    });
}

#[test]
fn classify_sta_ap_rx_never_panics_on_received_bytes() {
    classify_sta_ap_rx_never_panics(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn classify_sta_ap_rx_never_panics_on_received_bytes_full() {
    classify_sta_ap_rx_never_panics(oer_ieee80211_wire::fuzz::FULL);
}

#[test]
fn a_header_without_its_addresses_is_malformed() {
    let beacon = frame(8 << 4, GROUP, UPLINK, UPLINK);
    for length in [0, 1, 9, 15, 21, 22, 23] {
        assert_eq!(
            classify_sta_ap_rx(&beacon[..length], ADDRESSES),
            StaApRxRoute::Malformed
        );
    }
    assert_eq!(
        classify_sta_ap_rx(&beacon, ADDRESSES),
        StaApRxRoute::Interface(StaApVif::Station)
    );
    // An Ack needs only its receiver address; an RTS needs its TA as well.
    let ack = frame(CONTROL_FRAME | 0xd0, AP, PEER, PEER);
    assert_eq!(
        classify_sta_ap_rx(&ack[..10], ADDRESSES),
        StaApRxRoute::Interface(StaApVif::AccessPoint)
    );
    let rts = frame(CONTROL_FRAME | 0xb0, AP, PEER, PEER);
    assert_eq!(
        classify_sta_ap_rx(&rts[..15], ADDRESSES),
        StaApRxRoute::Malformed
    );
    assert_eq!(
        classify_sta_ap_rx(&rts[..16], ADDRESSES),
        StaApRxRoute::Interface(StaApVif::AccessPoint)
    );
}
