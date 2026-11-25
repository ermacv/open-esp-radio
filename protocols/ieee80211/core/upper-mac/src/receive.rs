//! One receive boundary for station Data and EAPOL.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::ccmp::{CcmpRxProtection, CcmpRxReplayEpoch};
use oer_protocols_ieee802_wire::{Eui48, Eui48Kind};
use oer_protocols_ieee80211_wire::{
    address::{Bssid, OwnAddress},
    ccmp::CcmpHeader,
    data::{MsduParts, decapsulate_payload},
    frame::DecodedHeader,
};

/// Why an RX frame or one of its MSDUs was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDropReason {
    Header,
    FrameKind,
    Address,
    Fragmented,
    Protection,
    Integrity,
    Replay,
    Payload,
    NotConnected,
}

/// RX integrity evidence projected by the port's consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Integrity {
    Unverified,
    DecryptedAndVerified,
}

/// A data MPDU admitted once, before demultiplexing its MSDUs by EtherType.
/// The private fields preserve its header, crypto and replay checks.
pub struct AdmittedData<'a> {
    header: DecodedHeader<'a>,
    payload: &'a [u8],
    protected: bool,
}
impl<'a> AdmittedData<'a> {
    /// Admit one infrastructure downlink. Protected MPDUs require a current
    /// replay owner and explicit integrity evidence; checking commits PN once.
    /// Null, QoS Null and other subtypes without an MSDU are rejected before
    /// replay admission, even when bytes resembling LLC/SNAP follow the header.
    pub fn admit(
        header: DecodedHeader<'a>,
        local: OwnAddress,
        bssid: Bssid,
        integrity: Integrity,
        replay: Option<&mut CcmpRxReplayEpoch>,
    ) -> Result<Self, RxDropReason> {
        let control = header.control();
        let fields = control.fields();
        if !control.is_data() || fields.no_data() || !fields.from_ds() || fields.to_ds() {
            return Err(RxDropReason::FrameKind);
        }
        let receiver = header.receiver();
        if header.transmitter() != bssid.get()
            || (receiver != local.get() && matches!(receiver.kind(), Eui48Kind::Individual(_)))
        {
            return Err(RxDropReason::Address);
        }
        if control.more_fragments() || header.sequence_control().fragment() != 0 {
            return Err(RxDropReason::Fragmented);
        }
        let mut payload = header.body();
        if control.protected() {
            if integrity != Integrity::DecryptedAndVerified {
                return Err(RxDropReason::Integrity);
            }
            let replay = replay.ok_or(RxDropReason::Protection)?;
            let (bytes, rest) = payload
                .split_first_chunk::<{ oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN }>()
                .ok_or(RxDropReason::Header)?;
            let ccmp = CcmpHeader::parse(*bytes).map_err(|_| RxDropReason::Header)?;
            let protection = if receiver == local.get() {
                CcmpRxProtection::Pairwise
            } else {
                CcmpRxProtection::Group
            };
            replay
                .check(protection, header.qos().map(|qos| qos.tid().get()), ccmp)
                .map_err(|_| RxDropReason::Replay)?;
            payload = rest;
        }
        Ok(Self {
            header,
            payload,
            protected: control.protected(),
        })
    }
    /// Admit one uplink of an access point's BSS: a To-DS data MPDU from
    /// `peer` addressed to the BSS. A protected MPDU requires explicit
    /// integrity evidence and the peer's replay owner, which checking
    /// advances once, under its pairwise lane; a peer sends nothing under
    /// the group key. Null, QoS Null and other subtypes without an MSDU are
    /// rejected before replay admission.
    pub fn admit_uplink(
        header: DecodedHeader<'a>,
        bssid: Bssid,
        peer: Eui48,
        integrity: Integrity,
        replay: Option<&mut CcmpRxReplayEpoch>,
    ) -> Result<Self, RxDropReason> {
        let control = header.control();
        let fields = control.fields();
        if !control.is_data() || fields.no_data() || fields.from_ds() || !fields.to_ds() {
            return Err(RxDropReason::FrameKind);
        }
        if header.receiver() != bssid.get() || header.transmitter() != peer {
            return Err(RxDropReason::Address);
        }
        if control.more_fragments() || header.sequence_control().fragment() != 0 {
            return Err(RxDropReason::Fragmented);
        }
        let mut payload = header.body();
        if control.protected() {
            if integrity != Integrity::DecryptedAndVerified {
                return Err(RxDropReason::Integrity);
            }
            let replay = replay.ok_or(RxDropReason::Protection)?;
            let (bytes, rest) = payload
                .split_first_chunk::<{ oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN }>()
                .ok_or(RxDropReason::Header)?;
            let ccmp = CcmpHeader::parse(*bytes).map_err(|_| RxDropReason::Header)?;
            replay
                .check(
                    CcmpRxProtection::Pairwise,
                    header.qos().map(|qos| qos.tid().get()),
                    ccmp,
                )
                .map_err(|_| RxDropReason::Replay)?;
            payload = rest;
        }
        Ok(Self {
            header,
            payload,
            protected: control.protected(),
        })
    }
    /// Whether this MPDU passed CCMP integrity and replay admission.
    pub const fn protected(&self) -> bool {
        self.protected
    }
    /// Decode LLC/SNAP or prevalidate all A-MSDU subframes before exposing any.
    pub fn msdus(
        &self,
    ) -> Result<
        impl Iterator<Item = Result<MsduParts<'a>, RxDropReason>> + 'a + use<'a>,
        RxDropReason,
    > {
        let frames =
            decapsulate_payload(self.header, self.payload).map_err(|_| RxDropReason::Payload)?;
        Ok(frames.map(|frame| frame.map_err(|_| RxDropReason::Payload)))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use oer_protocols_ieee802_wire::IndividualAddress;
    use oer_protocols_ieee80211_wire::ccmp::{CcmpKeyId, CcmpPacketNumber, GroupKeyId};
    use oer_protocols_ieee80211_wire::data::PeerQos;
    use oer_protocols_ieee80211_wire::security::LinkProtection;
    use std::vec::Vec;

    /// Whether a test frame carries an HT Control field.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum HtControl {
        Present,
        Absent,
    }

    fn packet(qos: PeerQos, ht: HtControl, protection: LinkProtection) -> Vec<u8> {
        let qos = qos == PeerQos::Qos;
        let ht = ht == HtControl::Present;
        let protected = protection == LinkProtection::Ccmp;
        let mut frame = std::vec![0; 24];
        let control: u16 = 0x0208
            | if qos { 0x80 } else { 0 }
            | if ht { 0x8000 } else { 0 }
            | if protected { 0x4000 } else { 0 };
        frame[..2].copy_from_slice(&control.to_le_bytes());
        frame[4..10].copy_from_slice(&[2; 6]);
        frame[10..16].copy_from_slice(&[4; 6]);
        frame[16..22].copy_from_slice(&[6; 6]);
        if qos {
            frame.extend_from_slice(&[7, 0]);
        }
        if ht {
            frame.extend_from_slice(&[0; 4]);
        }
        if protected {
            frame.extend_from_slice(
                &CcmpHeader::new(
                    CcmpPacketNumber::new(1).unwrap(),
                    CcmpKeyId::new(0).unwrap(),
                )
                .encode(),
            );
        }
        frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e, 1, 2, 3]);
        frame
    }
    fn admit<'a>(
        frame: &'a [u8],
        integrity: Integrity,
        replay: &mut CcmpRxReplayEpoch,
    ) -> Result<AdmittedData<'a>, RxDropReason> {
        AdmittedData::admit(
            DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?,
            OwnAddress::new(IndividualAddress::new(Eui48::new([2; 6])).unwrap()),
            Bssid::new(IndividualAddress::new(Eui48::new([4; 6])).unwrap()).unwrap(),
            integrity,
            Some(replay),
        )
    }
    /// An uplink of `peer` [4; 6] to the BSS [2; 6] for [6; 6].
    fn uplink(protection: LinkProtection, number: u64) -> Vec<u8> {
        let protected = protection == LinkProtection::Ccmp;
        let mut frame = std::vec![0; 24];
        let control: u16 = 0x0108 | if protected { 0x4000 } else { 0 };
        frame[..2].copy_from_slice(&control.to_le_bytes());
        frame[4..10].copy_from_slice(&[2; 6]);
        frame[10..16].copy_from_slice(&[4; 6]);
        frame[16..22].copy_from_slice(&[6; 6]);
        if protected {
            frame.extend_from_slice(
                &CcmpHeader::new(
                    CcmpPacketNumber::new(number).unwrap(),
                    CcmpKeyId::new(0).unwrap(),
                )
                .encode(),
            );
        }
        frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 9, 8]);
        frame
    }

    fn admit_uplink<'a>(
        frame: &'a [u8],
        peer: [u8; 6],
        integrity: Integrity,
        replay: Option<&mut CcmpRxReplayEpoch>,
    ) -> Result<AdmittedData<'a>, RxDropReason> {
        AdmittedData::admit_uplink(
            DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?,
            Bssid::new(IndividualAddress::new(Eui48::new([2; 6])).unwrap()).unwrap(),
            Eui48::new(peer),
            integrity,
            replay,
        )
    }

    #[test]
    fn an_uplink_names_its_peer_and_the_bss_and_passes_its_replay_owner_once() {
        let mut replay =
            CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
        let frame = uplink(LinkProtection::Ccmp, 5);
        // Another transmitter, a downlink and an unverified frame are refused.
        assert!(matches!(
            admit_uplink(
                &frame,
                [5; 6],
                Integrity::DecryptedAndVerified,
                Some(&mut replay)
            ),
            Err(RxDropReason::Address)
        ));
        assert!(matches!(
            admit_uplink(
                &packet(PeerQos::NonQos, HtControl::Absent, LinkProtection::Open),
                [4; 6],
                Integrity::Unverified,
                None
            ),
            Err(RxDropReason::FrameKind)
        ));
        assert!(matches!(
            admit_uplink(&frame, [4; 6], Integrity::Unverified, Some(&mut replay)),
            Err(RxDropReason::Integrity)
        ));
        let admitted = admit_uplink(
            &frame,
            [4; 6],
            Integrity::DecryptedAndVerified,
            Some(&mut replay),
        )
        .unwrap();
        assert!(admitted.protected());
        let msdu = admitted.msdus().unwrap().next().unwrap().unwrap();
        assert_eq!(msdu.ether_type, 0x0800);
        assert_eq!(msdu.payload, [9, 8]);
        assert_eq!(msdu.destination, Eui48::new([6; 6]));
        assert_eq!(msdu.source, Eui48::new([4; 6]));
        // The same packet number again is a replay.
        assert!(matches!(
            admit_uplink(
                &frame,
                [4; 6],
                Integrity::DecryptedAndVerified,
                Some(&mut replay)
            ),
            Err(RxDropReason::Replay)
        ));
        // An open uplink needs no replay owner.
        assert!(
            admit_uplink(
                &uplink(LinkProtection::Open, 0),
                [4; 6],
                Integrity::Unverified,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn a_downlink_is_the_bss_s_to_this_station_or_to_a_group() {
        let address = |at: usize, octets: [u8; 6]| {
            let mut frame = packet(PeerQos::NonQos, HtControl::Absent, LinkProtection::Open);
            frame[at..at + 6].copy_from_slice(&octets);
            let mut replay =
                CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
            admit(&frame, Integrity::Unverified, &mut replay).err()
        };
        assert_eq!(address(4, [2; 6]), None);
        assert_eq!(address(4, [0xff; 6]), None);
        assert_eq!(address(4, [6; 6]), Some(RxDropReason::Address));
        assert_eq!(address(10, [6; 6]), Some(RxDropReason::Address));
    }

    #[test]
    fn eapol_follows_the_checked_qos_ht_and_ccmp_boundary() {
        for (qos, ht) in [
            (PeerQos::NonQos, HtControl::Absent),
            (PeerQos::Qos, HtControl::Absent),
            (PeerQos::Qos, HtControl::Present),
        ] {
            for protected in [LinkProtection::Open, LinkProtection::Ccmp] {
                let frame = packet(qos, ht, protected);
                let mut replay =
                    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
                if protected == LinkProtection::Ccmp {
                    assert!(matches!(
                        admit(&frame, Integrity::Unverified, &mut replay),
                        Err(RxDropReason::Integrity)
                    ));
                }
                let admitted = admit(&frame, Integrity::DecryptedAndVerified, &mut replay).unwrap();
                let parts = admitted.msdus().unwrap().next().unwrap().unwrap();
                assert_eq!(parts.ether_type, 0x888e);
                assert_eq!(parts.payload, [1, 2, 3]);
                if protected == LinkProtection::Ccmp {
                    assert!(matches!(
                        admit(&frame, Integrity::DecryptedAndVerified, &mut replay),
                        Err(RxDropReason::Replay)
                    ));
                }
            }
        }
    }
    #[test]
    fn ccmp_integrity_does_not_admit_an_llc_aliased_amsdu() {
        let mut frame = packet(PeerQos::Qos, HtControl::Absent, LinkProtection::Ccmp);
        frame.truncate(26 + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN);
        frame[24] |= 0x80;
        frame.extend_from_slice(&[
            0xaa, 0xaa, 3, 0, 0, 0, // Ordinary MSDU LLC prefix, forged DA.
            0x08, 0, 0, 0, 0, 0, // Ordinary EtherType plus attacker data.
            0, 10, // Subframe length.
            0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0, 1, 2,
        ]);
        frame.extend_from_slice(&[
            2, 2, 2, 2, 2, 2, // Injected destination.
            6, 6, 6, 6, 6, 6, // Injected source.
            0, 10, 0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x06, 0xde, 0xad,
        ]);
        let mut replay =
            CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
        let admitted = admit(&frame, Integrity::DecryptedAndVerified, &mut replay).unwrap();
        assert!(admitted.protected());
        assert!(matches!(admitted.msdus(), Err(RxDropReason::Payload)));
        // The rejected payload cannot be retried with its already admitted PN.
        assert!(matches!(
            admit(&frame, Integrity::DecryptedAndVerified, &mut replay),
            Err(RxDropReason::Replay)
        ));
    }

    #[test]
    fn only_payload_bearing_data_subtypes_admit_msdus() {
        for subtype in 0..16 {
            for protected in [LinkProtection::Open, LinkProtection::Ccmp] {
                let mut frame = packet(
                    if subtype >= 8 {
                        PeerQos::Qos
                    } else {
                        PeerQos::NonQos
                    },
                    HtControl::Absent,
                    protected,
                );
                frame[0] = 0x08 | (subtype << 4);
                let mut replay =
                    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
                let result = admit(&frame, Integrity::DecryptedAndVerified, &mut replay);
                if matches!(subtype, 0..=3 | 8..=11) {
                    let admitted = result.unwrap();
                    let parts = admitted.msdus().unwrap().next().unwrap().unwrap();
                    assert_eq!(parts.ether_type, 0x888e);
                    assert_eq!(parts.payload, [1, 2, 3]);
                } else {
                    assert!(
                        matches!(result, Err(RxDropReason::FrameKind)),
                        "subtype {subtype}"
                    );
                    // Rejected Null/reserved bodies cannot consume the PN of
                    // a subsequent real data frame on the same replay lane.
                    frame[0] = if subtype >= 8 { 0x88 } else { 0x08 };
                    assert!(admit(&frame, Integrity::DecryptedAndVerified, &mut replay).is_ok());
                }
            }
        }
    }
    #[test]
    fn a_short_header_never_reaches_replay_admission() {
        let frame = packet(PeerQos::Qos, HtControl::Present, LinkProtection::Ccmp);
        for length in 0..38 {
            let mut replay =
                CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
            assert!(matches!(
                admit(
                    &frame[..length],
                    Integrity::DecryptedAndVerified,
                    &mut replay
                ),
                Err(RxDropReason::Header)
            ));
            assert!(admit(&frame, Integrity::DecryptedAndVerified, &mut replay).is_ok());
        }
    }

    /// Admit and demultiplex one input with each integrity verdict.
    fn admit_all(frame: &[u8]) {
        let Some(header) = DecodedHeader::decode(frame) else {
            return;
        };
        for integrity in [Integrity::Unverified, Integrity::DecryptedAndVerified] {
            let mut replay =
                CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
            let Ok(admitted) = AdmittedData::admit(
                header,
                OwnAddress::new(IndividualAddress::new(Eui48::new([2; 6])).unwrap()),
                Bssid::new(IndividualAddress::new(Eui48::new([4; 6])).unwrap()).unwrap(),
                integrity,
                Some(&mut replay),
            ) else {
                continue;
            };
            let _ = admitted.protected();
            if let Ok(msdus) = admitted.msdus() {
                for msdu in msdus {
                    let _ = msdu.map(|parts| parts.payload.len());
                }
            }
        }
    }

    fn run(count: usize) {
        let mut seeds = Vec::new();
        for (qos, ht) in [
            (PeerQos::NonQos, HtControl::Absent),
            (PeerQos::Qos, HtControl::Absent),
            (PeerQos::Qos, HtControl::Present),
        ] {
            for protected in [LinkProtection::Open, LinkProtection::Ccmp] {
                seeds.push(packet(qos, ht, protected));
            }
        }
        let mut amsdu = packet(PeerQos::Qos, HtControl::Absent, LinkProtection::Ccmp);
        amsdu.truncate(26 + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN);
        amsdu[24] |= 0x80;
        amsdu.extend_from_slice(&[2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 0, 10]);
        amsdu.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0, 1, 2]);
        amsdu.extend_from_slice(&[2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 0, 10]);
        amsdu.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x06, 0xde, 0xad]);
        seeds.push(amsdu);
        let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
        oer_protocols_ieee80211_wire::fuzz::inputs(&seeds, count, 96, admit_all);
    }

    #[test]
    fn admission_never_panics() {
        run(oer_protocols_ieee80211_wire::fuzz::QUICK);
    }

    #[test]
    #[ignore = "the full run"]
    fn admission_never_panics_full() {
        run(oer_protocols_ieee80211_wire::fuzz::FULL);
    }
}
