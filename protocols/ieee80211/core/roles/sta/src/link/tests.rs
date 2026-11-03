//! The link's receive admission, as its holder runs it without the station.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, RxCryptoStatus, RxEvidence, RxMeta,
};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
use std::vec::Vec;

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 3];

/// The receive admission of a protected link with `replay`.
fn protected_link(replay: CcmpRxReplayEpoch) -> RxLink {
    RxLink::new(
        OwnAddress::new(IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
        true,
        Some(replay),
    )
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap()
}

fn received_frame(control: u16, ether_type: u16, packet_number: u8, key_id: u8) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 26];
    frame[0..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    if control & 0x4000 != 0 {
        frame.extend_from_slice(&[packet_number, 0, 0, 0x20 | (key_id << 6), 0, 0, 0, 0]);
    }
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(&[0xcd; 20]);
    frame
}

fn meta(crypto: RxEvidence<RxCryptoStatus>) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
    meta.crypto = crypto;
    meta
}

fn decrypted() -> RxMeta {
    meta(RxEvidence::HardwareObserved(
        RxCryptoStatus::DecryptedAndIntegrityVerified,
    ))
}

/// What `link` takes from `frame`, which `meta` describes, while it
/// `carries` data: the network's MSDUs and the EAPOL packets it handed the
/// station, which answers `eapol`.
fn admitted<'f>(
    link: &mut RxLink,
    frame: &'f [u8],
    meta: &RxMeta,
    carries: bool,
    eapol: StaEapol,
) -> Result<(Vec<MsduDelivery<'f>>, Vec<&'f [u8]>), StaRxError> {
    let header = DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?;
    let mut data = Vec::new();
    let mut station = Vec::new();
    link.receive(
        header,
        frame,
        crate::data::integrity(meta.common()),
        carries,
        |delivery| data.push(delivery),
        |payload, _| {
            station.push(payload);
            Ok(eapol)
        },
    )?;
    Ok((data, station))
}

fn received<'f>(
    link: &mut RxLink,
    frame: &'f [u8],
    meta: &RxMeta,
) -> Result<Vec<MsduDelivery<'f>>, StaRxError> {
    admitted(link, frame, meta, true, StaEapol::Taken).map(|(data, _)| data)
}

#[test]
fn a_decrypted_data_mpdu_gives_its_msdu_and_an_unverified_or_eapol_one_none() {
    let mut link = protected_link(epoch());
    let frame = received_frame(0x4288, 0x0800, 1, 0);
    let parts = received(&mut link, &frame, &decrypted()).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].parts.source, PEER);
    assert_eq!(parts[0].parts.destination, LOCAL);
    assert_eq!(parts[0].parts.payload, &[0xcd; 20]);
    // Protected but not verified: nothing for the network.
    let frame = received_frame(0x4288, 0x0800, 2, 0);
    assert!(received(&mut link, &frame, &meta(RxEvidence::Unavailable)).is_err());
    // EAPOL is the station's, not the network's.
    let eapol = received_frame(0x4288, 0x888e, 3, 0);
    let (data, station) = admitted(&mut link, &eapol, &decrypted(), true, StaEapol::Taken).unwrap();
    assert!(data.is_empty());
    assert_eq!(station, [&[0xcd; 20][..]]);
}

#[test]
fn a_frame_sent_again_is_dropped_by_the_replay_check() {
    let mut link = protected_link(epoch());
    let frame = received_frame(0x4288, 0x0800, 7, 0);
    assert_eq!(received(&mut link, &frame, &decrypted()).unwrap().len(), 1);
    // The same packet number again: dropped before it reaches the network.
    assert_eq!(
        received(&mut link, &frame, &decrypted()),
        Err(RxDropReason::Replay.into())
    );
}

#[test]
fn the_old_and_the_new_group_key_are_both_taken_until_the_old_is_removed() {
    let mut link = protected_link(epoch());
    let group = |packet_number, key_id| {
        let mut frame = received_frame(0x4288, 0x0800, packet_number, key_id);
        frame[4..10].copy_from_slice(&[0xff; 6]);
        frame
    };
    assert_eq!(
        received(&mut link, &group(200, 1), &decrypted())
            .unwrap()
            .len(),
        1
    );
    // The access point's new group key, index 2, from packet number 1.
    assert!(
        link.replay_mut()
            .unwrap()
            .exchange_group(
                GroupKeyId::new(2).unwrap(),
                oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState::default(),
            )
            .is_none()
    );
    // Both keys are taken while both are installed.
    assert_eq!(
        received(&mut link, &group(1, 2), &decrypted())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        received(&mut link, &group(201, 1), &decrypted())
            .unwrap()
            .len(),
        1
    );
    link.replay_mut()
        .unwrap()
        .remove_group(GroupKeyId::new(1).unwrap());
    assert!(received(&mut link, &group(202, 1), &decrypted()).is_err());
    assert_eq!(
        received(&mut link, &group(2, 2), &decrypted())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_held_link_checks_replay_but_gives_no_data_and_eapol_still_goes_to_the_station() {
    let mut link = protected_link(epoch());
    let frame = received_frame(0x4288, 0x0800, 5, 0);
    // Held, the frame passes the replay check and gives the network nothing.
    assert_eq!(
        admitted(&mut link, &frame, &decrypted(), false, StaEapol::Taken),
        Err(RxDropReason::NotConnected.into())
    );
    // Released, the same packet number is a replay: the check ran.
    assert_eq!(
        received(&mut link, &frame, &decrypted()),
        Err(RxDropReason::Replay.into())
    );
    // The station's EAPOL goes to it while the link holds.
    let eapol = received_frame(0x4288, 0x888e, 6, 0);
    let (data, station) =
        admitted(&mut link, &eapol, &decrypted(), false, StaEapol::Taken).unwrap();
    assert!(data.is_empty());
    assert_eq!(station.len(), 1);
}

#[test]
fn a_failed_rsn_ends_the_links_data_from_the_next_msdu_on() {
    let mut link = protected_link(epoch());
    let eapol = received_frame(0x4288, 0x888e, 1, 0);
    let (_, station) = admitted(&mut link, &eapol, &decrypted(), true, StaEapol::Faulted).unwrap();
    assert_eq!(station.len(), 1);
    let frame = received_frame(0x4288, 0x0800, 2, 0);
    assert_eq!(
        received(&mut link, &frame, &decrypted()),
        Err(RxDropReason::NotConnected.into())
    );
}

#[test]
fn unprotected_data_of_a_protected_link_is_refused_and_foreign_open_eapol_goes_nowhere() {
    let mut link = protected_link(epoch());
    let data = received_frame(0x0288, 0x0800, 0, 0);
    assert_eq!(
        received(&mut link, &data, &meta(RxEvidence::Unavailable)),
        Err(RxDropReason::Protection.into())
    );
    // An open EAPOL frame to another station is not the station's.
    let mut foreign = received_frame(0x0288, 0x888e, 0, 0);
    foreign[4..10].copy_from_slice(&[0xff; 6]);
    let (_, station) = admitted(
        &mut link,
        &foreign,
        &meta(RxEvidence::Unavailable),
        true,
        StaEapol::Taken,
    )
    .unwrap();
    assert!(station.is_empty());
}

/// The link's holder hands EAPOL to the station in its own task: the
/// station's later RSN fault ends the link's data as the fault inside the
/// call does.
#[test]
fn a_fault_the_station_reports_later_ends_the_links_data() {
    let address = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    let mut link = LinkHandover::<
        '_,
        oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'_>>,
        (),
    >::for_test(1, address, bssid, false, false, None, None);
    link.hold(false);
    let frame = received_frame(0x0288, 0x0800, 0, 0);
    let deliveries = |link: &mut LinkHandover<'_, _, ()>| {
        let mut count = 0;
        let routed = link.receive(
            &frame,
            &meta(RxEvidence::Unavailable),
            |_| count += 1,
            |_, _| Ok(StaEapol::Taken),
        );
        (routed, count)
    };
    assert_eq!(
        deliveries(&mut link),
        (LinkRx::Taken(Ok(StaReceived::Data)), 1)
    );
    link.eapol_faulted();
    assert_eq!(
        deliveries(&mut link).0,
        LinkRx::Taken(Err(RxDropReason::NotConnected.into()))
    );
}
