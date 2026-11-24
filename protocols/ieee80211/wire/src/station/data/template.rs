//! A station's data MPDU prefix, built once per association.

use super::*;
use crate::data::{
    IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN, LLC_SNAP_HEADER_LEN,
    RFC1042_LLC_SNAP_PREFIX,
};
use crate::frame::MacHeader;

/// The prefix of every data MPDU a station sends to its access point on one
/// association: the MAC header, the CCMP header's room on a protected link
/// and the LLC/SNAP header. It is built once from what stays the same on the
/// association (the station's address, the BSSID, QoS, protection);
/// [`Self::write_before`] fills in what changes from one MPDU to the next
/// ([`StaDataHeaderFields`]). The written prefix is the one
/// [`StaOpenDataFrame`] and [`StaProtectedDataFrame`] encode with a zero CCMP
/// header, which the backend fills from its key.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format: To DS set,
/// Address 1 the BSSID, Address 2 the source, Address 3 the destination,
/// Sequence Control, QoS Control carrying the TID), 12.5.3.2 (CCMP MPDU
/// format: the eight-octet CCMP header follows the MAC header) and 5.1.4
/// (RFC 1042 LLC/SNAP encapsulation).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDataHeaderTemplate {
    /// The MAC header with a zero Address 3, Sequence Control and TID.
    header: MacHeader,
    protection: LinkProtection,
}

/// What changes from one data MPDU of an association to the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDataHeaderFields {
    /// Address 3: the MSDU's destination.
    pub destination: [u8; 6],
    /// The Sequence Number of the unfragmented MPDU.
    pub sequence_number: SequenceNumber,
    /// The TID of a QoS data MPDU; unused without QoS.
    pub priority: WmmUserPriority,
    /// The EtherType the LLC/SNAP header names.
    pub ether_type: u16,
}

impl StaDataHeaderTemplate {
    /// The template of the association of `source` with `bssid`, QoS data
    /// when the access point negotiated QoS; `None` for a BSSID a station
    /// sends no data to.
    pub fn new(
        source: OwnAddress,
        bssid: Bssid,
        peer_qos: PeerQos,
        protection: LinkProtection,
    ) -> Option<Self> {
        validate_peer(bssid.get()).ok()?;
        let plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            bssid.get(),
            source.get(),
            MsduHeader {
                destination: [0; 6],
                source: source.get(),
                ether_type: 0,
            },
            WmmUserPriority::UP0,
            peer_qos,
            QosAckPolicy::Normal,
        );
        let mut reader = oer_portable_wire::Reader::new(&plan.header);
        let header = MacHeader::decode(reader.take(usize::from(plan.header_len)).ok()?)?;
        Some(Self {
            header: header.with_protected(protection.frame_protection()),
            protection,
        })
    }

    /// Whether the access point negotiated QoS: QoS Data with a TID.
    pub const fn peer_qos(&self) -> PeerQos {
        if self.header.qos().is_some() {
            PeerQos::Qos
        } else {
            PeerQos::NonQos
        }
    }

    /// How the association's data is protected.
    pub const fn protection(&self) -> LinkProtection {
        self.protection
    }

    /// The BSSID the template's data goes to: Address 1.
    pub const fn bssid(&self) -> [u8; 6] {
        self.header.address1()
    }

    /// The station's own address: Address 2.
    pub const fn source(&self) -> [u8; 6] {
        self.header.address2()
    }

    /// Whether this is the template of these association inputs.
    pub fn serves(
        &self,
        source: OwnAddress,
        bssid: Bssid,
        peer_qos: PeerQos,
        protection: LinkProtection,
    ) -> bool {
        self.header.address1() == bssid.get()
            && self.header.address2() == source.get()
            && self.peer_qos() == peer_qos
            && self.protection == protection
    }

    /// The longest prefix a template writes: a QoS data header, the CCMP
    /// header and LLC/SNAP.
    pub const MAX_PREFIX_LEN: usize =
        IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;

    /// The length of the prefix [`Self::write_before`] writes.
    pub const fn prefix_len(&self) -> usize {
        const LEGACY_OPEN: usize = IEEE80211_LEGACY_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        const QOS_OPEN: usize = IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        const LEGACY_CCMP: usize =
            IEEE80211_LEGACY_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        match (self.peer_qos(), self.protection) {
            (PeerQos::NonQos, LinkProtection::Open) => LEGACY_OPEN,
            (PeerQos::Qos, LinkProtection::Open) => QOS_OPEN,
            (PeerQos::NonQos, LinkProtection::Ccmp) => LEGACY_CCMP,
            (PeerQos::Qos, LinkProtection::Ccmp) => Self::MAX_PREFIX_LEN,
        }
    }

    /// The MAC header of the MPDU of `fields`.
    fn header(&self, fields: StaDataHeaderFields) -> MacHeader {
        let header = self
            .header
            .with_address3(fields.destination)
            .with_sequence_control(fields.sequence_number.sequence_control());
        // User priorities are three-bit values; the TID field is four bits.
        header.map_qos(|qos| {
            qos.with_tid(oer_portable_wire::U8::<3>::from_low_bits(fields.priority.value()).widen())
        })
    }

    /// The prefix's octets: the MAC header, a zero CCMP header on a
    /// protected link (the backend writes the packet number and key ID) and
    /// LLC/SNAP naming `ether_type`; [`Self::prefix_len`] of them.
    fn octets(&self, header: &MacHeader, ether_type: u16) -> impl Iterator<Item = u8> + use<> {
        let ccmp = match self.protection {
            LinkProtection::Open => 0,
            LinkProtection::Ccmp => CCMP_HEADER_LEN,
        };
        header
            .octets()
            .chain(core::iter::repeat_n(0, ccmp))
            .chain(RFC1042_LLC_SNAP_PREFIX)
            .chain(ether_type.to_be_bytes())
    }

    /// Write the prefix of the MPDU of `fields` flush against the end of
    /// `headroom`, the octets in front of the MPDU's body, and return its
    /// MAC header; the prefix begins [`Self::prefix_len`] octets before the
    /// end. It cannot fail: `headroom` holds the longest prefix, which the
    /// build checks.
    ///
    /// ```compile_fail,E0080
    /// # use oer_protocols_ieee80211_wire::station::{StaDataHeaderFields, StaDataHeaderTemplate};
    /// fn short(template: &StaDataHeaderTemplate, fields: StaDataHeaderFields) {
    ///     let mut headroom = [0; StaDataHeaderTemplate::MAX_PREFIX_LEN - 1];
    ///     let _ = template.write_before(fields, &mut headroom);
    /// }
    /// // Instantiated, so its check runs.
    /// let _: fn(&StaDataHeaderTemplate, StaDataHeaderFields) = short;
    /// ```
    pub fn write_before<const N: usize>(
        &self,
        fields: StaDataHeaderFields,
        headroom: &mut [u8; N],
    ) -> MacHeader {
        const {
            if N < Self::MAX_PREFIX_LEN {
                panic!("the headroom holds the longest prefix");
            }
        }
        // The prefix is at most `MAX_PREFIX_LEN`, which `N` holds.
        let at = N.saturating_sub(self.prefix_len());
        let header = self.header(fields);
        let octets = self.octets(&header, fields.ether_type);
        for (place, octet) in headroom.iter_mut().skip(at).zip(octets) {
            *place = octet;
        }
        header
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::IndividualAddress;

    const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];

    fn source() -> OwnAddress {
        OwnAddress::new(IndividualAddress::new(LOCAL).unwrap())
    }

    fn bssid() -> Bssid {
        Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap()
    }

    /// Today's full encode of the same MPDU without payload: the oracle.
    fn encoded(
        peer_qos: PeerQos,
        protection: LinkProtection,
        fields: StaDataHeaderFields,
        output: &mut [u8],
    ) -> usize {
        match protection {
            LinkProtection::Open => StaOpenDataFrame {
                source: source(),
                bssid: bssid(),
                destination: fields.destination,
                sequence_number: fields.sequence_number,
                priority: fields.priority,
                peer_qos,
                ether_type: fields.ether_type,
                payload: &[],
            }
            .encode(output),
            LinkProtection::Ccmp => StaProtectedDataFrame {
                source: source(),
                bssid: bssid(),
                destination: fields.destination,
                sequence_number: fields.sequence_number,
                user_priority: fields.priority.value(),
                peer_qos,
                ccmp_header: [0; CCMP_HEADER_LEN],
                ether_type: fields.ether_type,
                payload: &[],
            }
            .encode(output),
        }
        .unwrap()
    }

    const MODES: [(PeerQos, LinkProtection); 4] = [
        (PeerQos::NonQos, LinkProtection::Open),
        (PeerQos::Qos, LinkProtection::Open),
        (PeerQos::NonQos, LinkProtection::Ccmp),
        (PeerQos::Qos, LinkProtection::Ccmp),
    ];

    #[test]
    fn a_template_names_its_bssid_and_its_source() {
        let template =
            StaDataHeaderTemplate::new(source(), bssid(), PeerQos::Qos, LinkProtection::Open)
                .unwrap();
        assert_eq!(template.bssid(), BSSID);
        assert_eq!(template.source(), LOCAL);
    }

    #[test]
    fn the_template_writes_the_bytes_and_header_of_the_full_encode_in_every_mode() {
        for (peer_qos, protection) in MODES {
            let template =
                StaDataHeaderTemplate::new(source(), bssid(), peer_qos, protection).unwrap();
            for priority in 0..8 {
                for sequence in [0, 1, 42, 2_048, 4_095] {
                    for (destination, ether_type) in [
                        ([2, 0, 0, 0, 0, 3], 0x0800),
                        ([0xff; 6], 0x0806),
                        ([0x33, 0x33, 0, 0, 0, 1], 0x86dd),
                        (BSSID, 0x888e),
                    ] {
                        let fields = StaDataHeaderFields {
                            destination,
                            sequence_number: SequenceNumber::new(sequence).unwrap(),
                            priority: WmmUserPriority::new(priority).unwrap(),
                            ether_type,
                        };
                        let mut expected = [0xee; 64];
                        let length = encoded(peer_qos, protection, fields, &mut expected);
                        let mut written = [0xee; 64];
                        let header = template.write_before(fields, &mut written);
                        assert_eq!(template.prefix_len(), length);
                        assert_eq!(written[64 - length..], expected[..length]);
                        assert!(written[..64 - length].iter().all(|octet| *octet == 0xee));
                        // The returned header is the one decoded from the
                        // full encode.
                        assert_eq!(Some(header), MacHeader::decode(&expected[..length]));
                    }
                }
            }
        }
    }

    #[test]
    fn each_write_replaces_the_sequence_tid_and_msdu_fields_of_the_last() {
        for (peer_qos, protection) in MODES {
            let template =
                StaDataHeaderTemplate::new(source(), bssid(), peer_qos, protection).unwrap();
            let first = StaDataHeaderFields {
                destination: [2, 0, 0, 0, 0, 3],
                sequence_number: SequenceNumber::new(4_095).unwrap(),
                priority: WmmUserPriority::UP7,
                ether_type: 0x86dd,
            };
            let second = StaDataHeaderFields {
                destination: [2, 0, 0, 0, 0, 4],
                sequence_number: SequenceNumber::new(1).unwrap(),
                priority: WmmUserPriority::UP2,
                ether_type: 0x0800,
            };
            let mut output = [0; 64];
            template.write_before(first, &mut output);
            let header = template.write_before(second, &mut output);
            assert_eq!(header.address3(), [2, 0, 0, 0, 0, 4]);
            assert_eq!(
                SequenceNumber::from_sequence_control(header.sequence_control()),
                SequenceNumber::new(1).unwrap()
            );
            assert_eq!(
                header.qos().map(|qos| qos.tid().get()),
                (peer_qos == PeerQos::Qos).then_some(2)
            );
            assert_eq!(
                header.control().protected(),
                protection == LinkProtection::Ccmp
            );
            let mut expected = [0; 64];
            let length = encoded(peer_qos, protection, second, &mut expected);
            assert_eq!(output[64 - length..], expected[..length]);
        }
    }

    #[test]
    fn a_write_before_puts_the_written_prefix_flush_against_the_body_in_every_mode() {
        for (peer_qos, protection) in MODES {
            let template =
                StaDataHeaderTemplate::new(source(), bssid(), peer_qos, protection).unwrap();
            let length = template.prefix_len();
            assert!(length <= StaDataHeaderTemplate::MAX_PREFIX_LEN);
            let fields = StaDataHeaderFields {
                destination: [2, 0, 0, 0, 0, 3],
                sequence_number: SequenceNumber::new(42).unwrap(),
                priority: WmmUserPriority::UP5,
                ether_type: 0x0800,
            };
            let mut expected = [0xee; 64];
            encoded(peer_qos, protection, fields, &mut expected);
            let mut headroom = [0xdd; 64];
            let header = template.write_before(fields, &mut headroom);
            assert_eq!(Some(header), MacHeader::decode(&expected[..length]));
            // The octets before the prefix keep what was there.
            assert!(headroom[..64 - length].iter().all(|octet| *octet == 0xdd));
            assert_eq!(headroom[64 - length..], expected[..length]);
            // The shortest headroom the build admits holds every prefix.
            let mut exact = [0xdd; StaDataHeaderTemplate::MAX_PREFIX_LEN];
            template.write_before(fields, &mut exact);
            let at = StaDataHeaderTemplate::MAX_PREFIX_LEN - length;
            assert_eq!(exact[at..], expected[..length]);
        }
    }

    #[test]
    fn a_template_serves_only_the_association_inputs_it_was_built_from() {
        let template =
            StaDataHeaderTemplate::new(source(), bssid(), PeerQos::Qos, LinkProtection::Ccmp)
                .unwrap();
        assert!(template.serves(source(), bssid(), PeerQos::Qos, LinkProtection::Ccmp));
        assert!(!template.serves(source(), bssid(), PeerQos::NonQos, LinkProtection::Ccmp));
        assert!(!template.serves(source(), bssid(), PeerQos::Qos, LinkProtection::Open));
        let other = OwnAddress::new(IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap());
        assert!(!template.serves(other, bssid(), PeerQos::Qos, LinkProtection::Ccmp));
        let other = Bssid::new(IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap()).unwrap();
        assert!(!template.serves(source(), other, PeerQos::Qos, LinkProtection::Ccmp));
    }
}
