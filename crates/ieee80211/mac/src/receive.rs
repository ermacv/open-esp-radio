//! One receive boundary for station Data and EAPOL.

use crate::{
    address::{Bssid, OwnAddress},
    ccmp::{CcmpHeader, CcmpRxProtection, CcmpRxReplayEpoch},
    data::{EthernetFrameParts, decapsulate_payload},
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
        if !control.is_data() || !matches!(control.subtype(), 0..=3 | 8..=11) || control.ds() != 2 {
            return Err(RxDropReason::FrameKind);
        }
        let receiver = header.receiver();
        if header.transmitter() != bssid.get() || (receiver != local.get() && receiver[0] & 1 == 0)
        {
            return Err(RxDropReason::Address);
        }
        if control.more_fragments() || header.sequence_control() & 15 != 0 {
            return Err(RxDropReason::Fragmented);
        }
        let mut payload = header.body();
        if control.protected() {
            if integrity != Integrity::DecryptedAndVerified {
                return Err(RxDropReason::Integrity);
            }
            let replay = replay.ok_or(RxDropReason::Protection)?;
            let (bytes, rest) = payload
                .split_first_chunk::<{ crate::ccmp::CCMP_HEADER_LEN }>()
                .ok_or(RxDropReason::Header)?;
            let ccmp = CcmpHeader::parse(*bytes).map_err(|_| RxDropReason::Header)?;
            let protection = if receiver == local.get() {
                CcmpRxProtection::Pairwise
            } else {
                CcmpRxProtection::Group
            };
            replay
                .check(protection, header.qos().map(|qos| (qos & 15) as u8), ccmp)
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
    /// Decode each LLC/SNAP or A-MSDU payload; malformed subframes stay errors.
    pub fn msdus(
        &self,
    ) -> Result<
        impl Iterator<Item = Result<EthernetFrameParts<'a>, RxDropReason>> + 'a + use<'a>,
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
    use crate::{
        address::IndividualAddress,
        ccmp::{CcmpKeyId, CcmpPacketNumber, GroupKeyId},
    };
    use std::vec::Vec;

    fn packet(qos: bool, ht: bool, protected: bool) -> Vec<u8> {
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
            OwnAddress::new(IndividualAddress::new([2; 6]).unwrap()),
            Bssid::new(IndividualAddress::new([4; 6]).unwrap()).unwrap(),
            integrity,
            Some(replay),
        )
    }
    #[test]
    fn eapol_follows_the_checked_qos_ht_and_ccmp_boundary() {
        for (qos, ht) in [(false, false), (true, false), (true, true)] {
            for protected in [false, true] {
                let frame = packet(qos, ht, protected);
                let mut replay =
                    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap();
                if protected {
                    assert!(matches!(
                        admit(&frame, Integrity::Unverified, &mut replay),
                        Err(RxDropReason::Integrity)
                    ));
                }
                let admitted = admit(&frame, Integrity::DecryptedAndVerified, &mut replay).unwrap();
                let parts = admitted.msdus().unwrap().next().unwrap().unwrap();
                assert_eq!(parts.ether_type, 0x888e);
                assert_eq!(parts.payload, [1, 2, 3]);
                if protected {
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
        let mut frame = packet(true, false, true);
        frame.truncate(26 + crate::ccmp::CCMP_HEADER_LEN);
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
            for protected in [false, true] {
                let mut frame = packet(subtype >= 8, false, protected);
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
        let frame = packet(true, true, true);
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
}
