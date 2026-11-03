//! The data module's receive admission and in-place header move.

use super::*;
use crate::epoch::LinkEpoch;
use crate::machine::{LinkMode, LinkProtection, LinkView, StaTokens};
use oer_protocols_ieee80211_contracts_port::association::KEY_BANK_CAPACITY;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth, RxEvidence};
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_wire::data::MsduParts;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use std::vec::Vec;
use {
    oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch,
    oer_protocols_ieee80211_wire::ccmp::GroupKeyId,
};

/// A transmit buffer of the test station's tokens.
struct Buffer;

impl oer_protocols_ieee80211_contracts_port::TxBufferClass for Buffer {
    const CAPACITY: usize = 1600;
}

impl oer_protocols_ieee80211_contracts_port::TxBufferMemory for Buffer {
    fn renew_msdu(
        &mut self,
    ) -> Result<(), oer_protocols_ieee80211_contracts_port::BufferMemoryUnavailable> {
        Ok(())
    }
    fn bytes_mut(
        &mut self,
    ) -> Result<&mut [u8], oer_protocols_ieee80211_contracts_port::BufferMemoryUnavailable> {
        Err(oer_protocols_ieee80211_contracts_port::BufferMemoryUnavailable)
    }
}

struct Station<'id>(core::marker::PhantomData<fn(&'id ()) -> &'id ()>);

impl<'id> StaTokens for Station<'id> {
    type AssociationStart = ();
    type Association = ();
    type VacantKeyBank = oer_protocols_ieee80211_contracts_port::association::KeyBankParts<
        (),
        (),
        KEY_BANK_CAPACITY,
    >;
    type VacantAssociation = [(); 0];
    fn begin_association(_: (), _: oer_protocols_ieee80211_wire::security::Spp) {}
    fn association(key: &()) -> &() {
        key
    }
    fn split_vacant(group: Self::VacantAssociation) -> ((), (), Self::VacantKeyBank) {
        (
            (),
            (),
            oer_protocols_ieee80211_contracts_port::association::KeyBankParts {
                pairwise: heapless::Vec::new(),
                group: group.into_iter().collect(),
            },
        )
    }
    fn collect_key_bank(
        parts: Self::VacantKeyBank,
    ) -> Result<Self::VacantKeyBank, Self::VacantKeyBank> {
        Ok(parts)
    }
    fn end_association(_: (), mut bank: Self::VacantKeyBank) -> ((), (), Self::VacantKeyBank) {
        bank.pairwise.pop();
        ((), (), bank)
    }
    const PAIRWISE_SLOTS: usize = 1;
    const TOTAL_GROUP_SLOTS: usize = 0;
    const GROUP_SLOTS: usize = 0;
    type GatePath = oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'id>>;
    type EpochPath = oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'id>>;
    type ConfigurationPath = oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'id>>;
    type Small = Buffer;
    type Ticket = u8;
    type SmallBuffer = Buffer;
    type Sent = Buffer;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        mpdu
    }
    type PairwiseSlot = ();
    type GroupSlot = ();
    type PairwiseKey = ();
    type GroupKey = ();
    type GroupPermission = ();
    type RxMeta = RxMeta;
}

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 3];

fn qos_link<'a>(
    key: &'a (),
    replay: &'a mut CcmpRxReplayEpoch,
) -> LinkView<'a, 'static, Station<'static>> {
    LinkView {
        epoch: LinkEpoch::for_test(1),
        address: oer_protocols_ieee80211_wire::address::OwnAddress::new(
            oer_protocols_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: oer_protocols_ieee80211_wire::address::Bssid::new(
            oer_protocols_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        association_id: 1,
        // The station's QoS sequence spaces, which the link lends.
        mode: LinkMode::Qos(std::boxed::Box::leak(std::boxed::Box::new(
            [SequenceCounter::new(SequenceNumber::ZERO); 8],
        ))),
        admission: AcAdmission::OPEN,
        protection: LinkProtection::Ccmp { key, replay },
    }
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

#[test]
fn a_decrypted_data_mpdu_gives_its_msdu_and_an_unverified_or_eapol_one_none() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 1, 0);
    let parts: Vec<_> = received(&mut link, &frame, &decrypted())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].source, PEER);
    assert_eq!(parts[0].destination, LOCAL);
    assert_eq!(parts[0].payload, &[0xcd; 20]);
    // Protected but not verified: nothing for the network.
    let frame = received_frame(0x4288, 0x0800, 2, 0);
    assert!(received(&mut link, &frame, &meta(RxEvidence::Unavailable)).is_err());
    // EAPOL is the station's.
    let eapol = received_frame(0x4288, 0x888e, 3, 0);
    assert_eq!(
        received(&mut link, &eapol, &decrypted()).unwrap().count(),
        0
    );
}

#[test]
fn a_frame_sent_again_is_dropped_by_the_replay_check() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 7, 0);
    assert_eq!(
        received(&mut link, &frame, &decrypted()).unwrap().count(),
        1
    );
    // The same packet number again: dropped before it reaches the network.
    assert!(received(&mut link, &frame, &decrypted()).is_err());
}

#[test]
fn the_old_and_the_new_group_key_are_both_taken_until_the_old_is_removed() {
    let mut replay = epoch();
    let group = |packet_number, key_id| {
        let mut frame = received_frame(0x4288, 0x0800, packet_number, key_id);
        frame[4..10].copy_from_slice(&[0xff; 6]);
        frame
    };
    {
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(200, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    // The access point's new group key, index 2, from packet number 1.
    assert!(
        replay
            .exchange_group(
                GroupKeyId::new(2).unwrap(),
                oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState::default(),
            )
            .is_none()
    );
    {
        // Both keys are taken while both are installed.
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(1, 2), &decrypted())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            received(&mut link, &group(201, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    replay.remove_group(GroupKeyId::new(1).unwrap());
    let mut link = qos_link(&(), &mut replay);
    assert!(received(&mut link, &group(202, 1), &decrypted()).is_err());
    assert_eq!(
        received(&mut link, &group(2, 2), &decrypted())
            .unwrap()
            .count(),
        1
    );
}

const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// The MSDUs of a data MPDU of `link`'s BSS to the station, which `meta`
/// describes: a protected one only once the hardware decrypted and verified
/// it and the replay check passed it. An EAPOL frame is the station's, not
/// the network's.
fn received<'f, T: StaTokens>(
    link: &mut LinkView<'_, '_, T>,
    frame: &'f [u8],
    meta: &RxMeta,
) -> Result<
    impl Iterator<
        Item = Result<MsduParts<'f>, oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason>,
    > + 'f,
    oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason,
> {
    use oer_protocols_ieee80211_core_upper_mac::receive::{AdmittedData, RxDropReason};
    let protected_link = matches!(link.protection, LinkProtection::Ccmp { .. });
    let replay = match &mut link.protection {
        LinkProtection::Ccmp { replay, .. } => Some(&mut **replay),
        LinkProtection::Open => None,
    };
    let admitted = AdmittedData::admit(
        oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(frame)
            .ok_or(RxDropReason::Header)?,
        link.address,
        link.bssid,
        integrity(meta),
        replay,
    )?;
    if protected_link != admitted.protected() {
        return Err(RxDropReason::Protection);
    }
    Ok(admitted.msdus()?.filter(|parts| match parts {
        Ok(parts) => parts.ether_type != EAPOL_ETHER_TYPE,
        Err(_) => true,
    }))
}

#[test]
fn a_shorter_header_moves_the_payload_down_to_it_without_overlap() {
    let mut region = std::vec![0_u8; 200];
    for (index, octet) in region[MSDU_PAYLOAD_OFFSET..MSDU_PAYLOAD_OFFSET + 50]
        .iter_mut()
        .enumerate()
    {
        *octet = index as u8;
    }
    let end = MSDU_PAYLOAD_OFFSET + 50;
    for to in [0, 1, MSDU_PAYLOAD_OFFSET - 1] {
        let mut moved = region.clone();
        move_down(&mut moved, to, end).unwrap();
        assert_eq!(&moved[to..to + 50], &region[MSDU_PAYLOAD_OFFSET..end]);
    }
    assert!(move_down(&mut region, 0, 201).is_none());
}
