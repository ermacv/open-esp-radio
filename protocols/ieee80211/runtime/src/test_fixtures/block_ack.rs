//! The fixtures of the receive and transmit Block Ack cases: an HT station,
//! its access point's beacon and the Block Ack action frames.

use std::vec::Vec;

use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth};
use oer_protocols_ieee80211_core_roles_sta::machine::StaSecurity;
use oer_protocols_ieee80211_testing_model::AirFrame;
use oer_protocols_ieee80211_wire::{
    block_ack::frame::{
        BlockAckAction, DelbaParameters, encode_addba_request, parse_block_ack_action,
    },
    ht::{HtLocalCapabilities, ht_capability_ie},
    sequence::SequenceNumber,
    station::AssociationCapabilities,
};

use super::{BSSID, LOCAL, TestConfig, beacon, config};

const HT: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(0, 0, 0xff, 1),
    match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("fixed HT20 test channel"),
    },
);
static HT_ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT,
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: None,
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};

pub(crate) fn ht_config() -> TestConfig {
    let (mut configuration, security) = config(StaSecurity::Open);
    configuration.phy = oer_protocols_ieee80211_core_roles_sta::association::StaPhyRequest::Ht20;
    configuration.association = &HT_ASSOCIATION;
    (configuration, security)
}

pub(crate) fn ht_beacon() -> Vec<u8> {
    let mut frame = beacon(false);
    frame.extend_from_slice(&HT);
    frame.extend_from_slice(&HT_ASSOCIATION.wmm);
    frame.extend_from_slice(&[
        61, 22, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    frame
}

pub(crate) fn management(body: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0; 24];
    frame[0] = 0xd0;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(body);
    frame
}

pub(crate) fn addba(dialog: u8, tid: u8, start: u16, timeout: u16) -> Vec<u8> {
    management(
        &encode_addba_request(
            dialog,
            tid,
            64,
            timeout,
            true,
            SequenceNumber::new(start).unwrap(),
        )
        .unwrap(),
    )
}

pub(crate) fn delba(tid: u8, initiator: bool) -> Vec<u8> {
    let [a, b] = DelbaParameters::from_bytes([0; 2])
        .with_tid(tid)
        .unwrap()
        .with_initiator(initiator)
        .to_bytes();
    management(&[3, 2, a, b, 37, 0])
}

pub(crate) fn data(sequence: u16, tid: u8, marker: u8) -> Vec<u8> {
    let mut frame = std::vec![0; 26];
    frame[..2].copy_from_slice(&0x0288u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[22..24].copy_from_slice(
        &SequenceNumber::new(sequence)
            .unwrap()
            .sequence_control()
            .to_le_bytes(),
    );
    frame[24] = tid;
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 8, 0, marker]);
    frame
}

pub(crate) fn response(frame: &AirFrame) -> BlockAckAction {
    parse_block_ack_action(&frame.plaintext[24..]).unwrap()
}

pub(crate) fn tx_block_ack_config((mut configuration, security): TestConfig) -> TestConfig {
    configuration.tx_block_ack = Some(
        oer_protocols_ieee80211_core_roles_sta::machine::StaTxBlockAck::new(
            oer_protocols_ieee80211_core_roles_sta::machine::StaTxBlockAckPolicy {
                window: 32,
                negotiation_timeout: Duration::from_millis(100),
                retry: oer_protocols_ieee80211_core_upper_mac::block_ack::TxBlockAckRetry {
                    attempts: 2,
                    interval: Duration::from_millis(50),
                },
                inactivity: Duration::from_secs(1),
                retry_members: oer_protocols_ieee80211_core_roles_sta::data::AggregateRetry {
                    lifetime: Duration::from_millis(1_500),
                    attempts: 4,
                },
            },
        )
        .unwrap(),
    );
    (configuration, security)
}

pub(crate) fn addba_response(dialog_token: u8, tid: u8, window: u16) -> Vec<u8> {
    management(
        &oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaResponse {
            dialog_token,
            status: oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaStatus::Success,
            tid,
            window,
            timeout_tu: 0,
            amsdu: false,
        }
        .encode()
        .unwrap(),
    )
}
