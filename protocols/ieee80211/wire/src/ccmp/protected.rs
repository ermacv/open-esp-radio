//! A CCMP-protected MPDU: its MAC header, CCMP header, protected body and
//! MIC, and the additional authentication data and nonce CCM takes.
//!
//! SOURCE: IEEE Std 802.11-2020, 12.5.3.3.3 (construct AAD: masked Frame
//! Control, the addresses, masked Sequence Control, Address 4 and masked QoS
//! Control) and 12.5.3.3.4 (construct CCM nonce: Nonce Flags, A2, PN). With
//! SPP A-MSDU negotiated the A-MSDU Present bit is not masked (12.5.3.3.3).
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use super::{CCMP_HEADER_LEN, CcmpHeader, CcmpHeaderError};
use crate::frame::{DecodedHeader, FrameControlFields, QosControl};
use crate::security::Spp;
use crate::sequence::SequenceControl;

/// Length of the CCMP-128 MIC.
pub const CCMP_MIC_LEN: usize = 8;
/// Length of the CCM nonce.
pub const CCMP_NONCE_LEN: usize = 13;

/// A protected MPDU that cannot be one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedMpduError {
    /// The MAC header is malformed.
    MalformedHeader,
    /// The Protected Frame bit is clear.
    NotProtected,
    /// The CCMP header is invalid.
    InvalidCcmpHeader(CcmpHeaderError),
    /// Shorter than its headers and MIC.
    Truncated,
    /// The protected body exceeds the CCM length field.
    TooLong,
}

/// The additional authentication data of one protected MPDU: 22 to 30
/// octets, as its header's shape decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpAad {
    bytes: [u8; 30],
    length: oer_wire::PrefixLength<30>,
}

impl CcmpAad {
    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }
}

/// A checked CCMP-protected MPDU over the caller's bytes.
#[derive(Clone, Copy, Debug)]
pub struct ProtectedMpdu<'a> {
    header: DecodedHeader<'a>,
    ccmp: CcmpHeader,
    body_start: usize,
    body_len: usize,
}

impl<'a> ProtectedMpdu<'a> {
    /// Check the MPDU's MAC header, its CCMP header and room for its MIC.
    pub fn parse(frame: &'a [u8]) -> Result<Self, ProtectedMpduError> {
        let header = DecodedHeader::decode(frame).ok_or(ProtectedMpduError::MalformedHeader)?;
        if !header.control().protected() {
            return Err(ProtectedMpduError::NotProtected);
        }
        let (ccmp, protected) = header
            .body()
            .split_first_chunk::<CCMP_HEADER_LEN>()
            .ok_or(ProtectedMpduError::Truncated)?;
        let ccmp = CcmpHeader::parse(*ccmp).map_err(ProtectedMpduError::InvalidCcmpHeader)?;
        let body_len = protected
            .len()
            .checked_sub(CCMP_MIC_LEN)
            .ok_or(ProtectedMpduError::Truncated)?;
        u16::try_from(body_len).map_err(|_| ProtectedMpduError::TooLong)?;
        let body_start = frame
            .len()
            .checked_sub(protected.len())
            .ok_or(ProtectedMpduError::Truncated)?;
        Ok(Self {
            header,
            ccmp,
            body_start,
            body_len,
        })
    }

    pub const fn header(&self) -> DecodedHeader<'a> {
        self.header
    }

    pub const fn ccmp_header(&self) -> CcmpHeader {
        self.ccmp
    }

    /// Where the protected body starts in the MPDU and how long it is; the
    /// MIC follows it.
    pub const fn body_range(&self) -> (usize, usize) {
        (self.body_start, self.body_len)
    }

    /// The CCM nonce: Nonce Flags, the transmitter address and the packet
    /// number, most significant octet first.
    pub fn nonce(&self, spp: Spp) -> [u8; CCMP_NONCE_LEN] {
        let [a0, a1, a2, a3, a4, a5] = self.header.transmitter();
        let [_, _, p0, p1, p2, p3, p4, p5] = self.ccmp.packet_number().value().to_be_bytes();
        [
            self.nonce_flags(spp),
            a0,
            a1,
            a2,
            a3,
            a4,
            a5,
            p0,
            p1,
            p2,
            p3,
            p4,
            p5,
        ]
    }

    /// The additional authentication data; `None` only when its parts
    /// exceed the two CCM blocks, which a checked header cannot.
    pub fn aad(&self, spp: Spp) -> Option<CcmpAad> {
        let control = self.header.control();
        // Retry, Power Management and More Data are masked in every frame;
        // a data frame also masks its subtype bits 4 to 6, QoS data its
        // Order bit. Protected stays set.
        let fields = FrameControlFields::from_bytes(control.get().to_le_bytes())
            .with_retry(false)
            .with_power_management(false)
            .with_more_data(false);
        let fields = if control.is_data() {
            fields
                .with_cf_ack(false)
                .with_cf_poll(false)
                .with_no_data(false)
        } else {
            fields
        };
        let fields = if control.is_qos_data() {
            fields.with_order(false)
        } else {
            fields
        };
        let sequence = SequenceControl::from_bytes(self.header.sequence_control().to_le_bytes());
        let address4 = self.header.address4();
        let qos = self.masked_qos(spp).map(|qos| qos.to_bytes());
        let mut bytes = [0; 30];
        let mut writer = oer_wire::Writer::new(&mut bytes);
        for part in [
            fields.to_bytes().as_slice(),
            &self.header.receiver(),
            &self.header.transmitter(),
            &self.header.address3(),
            // The Fragment Number, the Sequence Number masked to zero.
            &SequenceControl::from_bytes([0; 2])
                .with_fragment(sequence.fragment())
                .ok()?
                .to_bytes(),
            address4.as_ref().map_or(&[], |address| address.as_slice()),
            qos.as_ref().map_or(&[], |qos| qos.as_slice()),
        ] {
            writer.write(part).ok()?;
        }
        let length = oer_wire::PrefixLength::new(writer.written())?;
        Some(CcmpAad { bytes, length })
    }

    /// The QoS Control field with the bits AAD excludes masked: the TID
    /// remains, and the A-MSDU Present bit when SPP is negotiated.
    fn masked_qos(&self, spp: Spp) -> Option<QosControl> {
        self.header.qos().map(|qos| {
            let control = QosControl::from_bytes(qos.to_le_bytes());
            QosControl::from_bytes([0; 2])
                .with_tid(control.tid())
                .with_amsdu(matches!(spp, Spp::Negotiated) && control.amsdu())
        })
    }

    fn nonce_flags(&self, spp: Spp) -> u8 {
        let flags = match self.masked_qos(spp) {
            Some(qos) => CcmNonceFlags::from_bytes([0])
                .with_priority(qos.tid())
                .with_amsdu(qos.amsdu()),
            None => {
                CcmNonceFlags::from_bytes([0]).with_management(!self.header.control().is_data())
            }
        };
        let [flags] = flags.to_bytes();
        flags
    }
}

oer_wire::bitfield! {
    /// The CCM Nonce Flags octet: the priority (the TID of QoS data), the
    /// management bit, and with SPP A-MSDU the A-MSDU Present bit.
    /// SOURCE: IEEE Std 802.11-2020, 12.5.3.3.4 (Construct CCM nonce,
    /// Figure 12-18); with SPP the A-MSDU Present bit joins the priority as
    /// Linux v6.12 `ccmp_special_blocks` does.
    pub struct CcmNonceFlags[1] (little) {
        pub priority, with_priority: (u8) = 0..4;
        pub management, with_management: bool = 4..5;
        pub amsdu, with_amsdu: bool = 7..8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A protected QoS data MPDU to the DS with Retry, Power Management and
    /// More Data set, Sequence Control 0x1234, QoS Control TID 5 with
    /// A-MSDU Present, PN 0x0102_0304_0506 and a two-octet body.
    fn qos_data() -> std::vec::Vec<u8> {
        let mut frame = std::vec![0x88, 0x79, 0, 0];
        frame.extend_from_slice(&[0x10; 6]);
        frame.extend_from_slice(&[0x20; 6]);
        frame.extend_from_slice(&[0x30; 6]);
        frame.extend_from_slice(&0x1234_u16.to_le_bytes());
        frame.extend_from_slice(&[0x85, 0x00]);
        // CCMP header: PN0, PN1, reserved, Ext IV with key 0, PN2..PN5.
        frame.extend_from_slice(&[0x06, 0x05, 0, 0x20, 0x04, 0x03, 0x02, 0x01]);
        frame.extend_from_slice(&[0xaa, 0xbb]);
        frame.extend_from_slice(&[0; CCMP_MIC_LEN]);
        frame
    }

    #[test]
    fn qos_data_aad_masks_the_mutable_bits_and_spp_keeps_amsdu_present() {
        let frame = qos_data();
        let mpdu = ProtectedMpdu::parse(&frame).unwrap();
        assert_eq!(mpdu.body_range(), (34, 2));
        let mut expected = std::vec![0x88, 0x41];
        expected.extend_from_slice(&[0x10; 6]);
        expected.extend_from_slice(&[0x20; 6]);
        expected.extend_from_slice(&[0x30; 6]);
        expected.extend_from_slice(&[0x04, 0x00, 0x05, 0x00]);
        assert_eq!(mpdu.aad(Spp::NotNegotiated).unwrap().as_bytes(), expected);
        let last = expected.len() - 2;
        expected[last] = 0x85;
        assert_eq!(mpdu.aad(Spp::Negotiated).unwrap().as_bytes(), expected);
        let mut nonce = [0x05, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 1, 2, 3, 4, 5, 6];
        assert_eq!(mpdu.nonce(Spp::NotNegotiated), nonce);
        nonce[0] = 0x85;
        assert_eq!(mpdu.nonce(Spp::Negotiated), nonce);
    }

    #[test]
    fn management_aad_keeps_the_subtype_and_its_nonce_names_management() {
        let mut frame = std::vec![0xd0, 0x48, 0, 0];
        frame.extend_from_slice(&[0x10; 6]);
        frame.extend_from_slice(&[0x20; 6]);
        frame.extend_from_slice(&[0x30; 6]);
        frame.extend_from_slice(&[0x10, 0x00]);
        frame.extend_from_slice(&[0x01, 0, 0, 0x20, 0, 0, 0, 0]);
        frame.extend_from_slice(&[0; CCMP_MIC_LEN]);
        let mpdu = ProtectedMpdu::parse(&frame).unwrap();
        assert_eq!(mpdu.body_range(), (32, 0));
        let aad = mpdu.aad(Spp::Negotiated).unwrap();
        assert_eq!(&aad.as_bytes()[..2], &[0xd0, 0x40]);
        assert_eq!(aad.as_bytes().len(), 22);
        assert_eq!(mpdu.nonce(Spp::Negotiated)[0], 0x10);
    }

    #[test]
    fn an_mpdu_refuses_what_cannot_be_protected() {
        let frame = qos_data();
        let mut open = frame.clone();
        open[1] &= !0x40;
        assert_eq!(
            ProtectedMpdu::parse(&open).err(),
            Some(ProtectedMpduError::NotProtected)
        );
        assert_eq!(
            ProtectedMpdu::parse(&frame[..33]).err(),
            Some(ProtectedMpduError::Truncated)
        );
        let mut no_ext_iv = frame.clone();
        no_ext_iv[29] = 0;
        assert!(matches!(
            ProtectedMpdu::parse(&no_ext_iv),
            Err(ProtectedMpduError::InvalidCcmpHeader(_))
        ));
        assert_eq!(
            ProtectedMpdu::parse(&frame[..10]).err(),
            Some(ProtectedMpduError::MalformedHeader)
        );
    }
}
