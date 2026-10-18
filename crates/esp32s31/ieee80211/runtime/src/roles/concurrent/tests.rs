use oer_ieee80211_mac::sequence::SequenceNumber;

use super::*;

const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];

#[test]
fn logical_vifs_lower_to_their_exact_hardware_contexts() {
    assert_eq!(lower_sta_ap_vif(StaApVif::Station), MacInterface::Station);
    assert_eq!(
        lower_sta_ap_vif(StaApVif::AccessPoint),
        MacInterface::AccessPoint
    );
    assert_ne!(
        network_interface_id(StaApVif::Station),
        network_interface_id(StaApVif::AccessPoint)
    );
    assert_eq!(
        sta_ap_vif(STA_NETWORK_INTERFACE_ID),
        Some(StaApVif::Station)
    );
    assert_eq!(sta_ap_vif(NetworkInterfaceId::new(2)), None);
}

#[test]
fn shared_rx_block_ack_owner_allocates_distinct_station_and_ap_banks() {
    let sessions = StaApRxBlockAck::with_maximum_window(16).unwrap();
    for (interface, peer) in [
        (MacInterface::Station, UPLINK),
        (MacInterface::AccessPoint, PEER),
    ] {
        sessions
            .offer(RxBlockAckRequest {
                interface,
                peer,
                dialog_token: 1,
                tid: 0,
                immediate: true,
                requested_window: 16,
                timeout_tu: 0,
                starting_sequence: SequenceNumber::new(7).unwrap(),
            })
            .unwrap();
        let activation = sessions.begin_pending().unwrap().unwrap();
        sessions.commit(activation).unwrap();
    }

    let station = sessions.snapshots_for(MacInterface::Station)[0].unwrap();
    let access_point = sessions.snapshots_for(MacInterface::AccessPoint)[1].unwrap();
    assert_eq!(station.hardware_index, 0);
    assert_eq!(access_point.hardware_index, 1);
}

#[test]
fn shared_tx_dispatch_retains_the_exact_owner_for_every_tag() {
    let station = TaggedStableDmaBacking::new(STA_NETWORK_INTERFACE_ID, 11_u8);
    let access_point = TaggedStableDmaBacking::new(AP_NETWORK_INTERFACE_ID, 22_u8);
    let unknown = TaggedStableDmaBacking::new(NetworkInterfaceId::new(9), 33_u8);

    assert!(matches!(
        dispatch_sta_ap_tx(station),
        StaApTxDispatch::Station(_)
    ));
    assert!(matches!(
        dispatch_sta_ap_tx(access_point),
        StaApTxDispatch::AccessPoint(_)
    ));
    let StaApTxDispatch::Unknown(owner) = dispatch_sta_ap_tx(unknown) else {
        panic!("unknown interface must fail closed")
    };
    assert_eq!(*owner.tag(), NetworkInterfaceId::new(9));
    assert_eq!(*owner, 33);
}
