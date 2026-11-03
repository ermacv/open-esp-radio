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
/// [`Self::write`] fills in what changes from one MPDU to the next
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
        peer_qos: bool,
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
            false,
        );
        let mut reader = oer_portable_wire::Reader::new(&plan.header);
        let header = MacHeader::decode(reader.take(usize::from(plan.header_len)).ok()?)?;
        Some(Self {
            header: header.with_protected(matches!(protection, LinkProtection::Ccmp)),
            protection,
        })
    }

    /// Whether the access point negotiated QoS: QoS Data with a TID.
    pub const fn peer_qos(&self) -> bool {
        self.header.qos().is_some()
    }

    /// How the association's data is protected.
    pub const fn protection(&self) -> LinkProtection {
        self.protection
    }

    /// Whether this is the template of these association inputs.
    pub fn serves(
        &self,
        source: OwnAddress,
        bssid: Bssid,
        peer_qos: bool,
        protection: LinkProtection,
    ) -> bool {
        self.header.address1() == bssid.get()
            && self.header.address2() == source.get()
            && self.peer_qos() == peer_qos
            && self.protection == protection
    }

    /// The length of the prefix [`Self::write`] writes.
    pub const fn prefix_len(&self) -> usize {
        const LEGACY_OPEN: usize = IEEE80211_LEGACY_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        const QOS_OPEN: usize = IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        const LEGACY_CCMP: usize =
            IEEE80211_LEGACY_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        const QOS_CCMP: usize =
            IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        match (self.peer_qos(), self.protection) {
            (false, LinkProtection::Open) => LEGACY_OPEN,
            (true, LinkProtection::Open) => QOS_OPEN,
            (false, LinkProtection::Ccmp) => LEGACY_CCMP,
            (true, LinkProtection::Ccmp) => QOS_CCMP,
        }
    }

    /// Write the prefix of the MPDU of `fields` at the start of `output`
    /// and return its MAC header. A shorter `output` is refused before any
    /// octet is written.
    pub fn write(
        &self,
        fields: StaDataHeaderFields,
        output: &mut [u8],
    ) -> Result<MacHeader, StationFrameError> {
        let required = self.prefix_len();
        let too_small = StationFrameError::OutputTooSmall { required };
        let prefix = output.get_mut(..required).ok_or(too_small)?;
        let header = self
            .header
            .with_address3(fields.destination)
            .with_sequence_control(fields.sequence_number.sequence_control());
        let header = match header.qos() {
            // User priorities are three-bit values; the TID field is four
            // bits.
            Some(qos) => header
                .with_qos(qos.with_tid(
                    oer_portable_wire::U8::<3>::from_low_bits(fields.priority.value()).widen(),
                ))
                .unwrap_or(header),
            None => header,
        };
        let ccmp: &[u8] = match self.protection {
            LinkProtection::Open => &[],
            // The backend writes the packet number and key ID.
            LinkProtection::Ccmp => &[0; CCMP_HEADER_LEN],
        };
        let [p0, p1, p2, p3, p4, p5] = RFC1042_LLC_SNAP_PREFIX;
        let [e0, e1] = fields.ether_type.to_be_bytes();
        let written = header.encode(prefix).map_err(|_| too_small)?;
        let mut writer =
            oer_portable_wire::Writer::new(prefix.get_mut(written..).ok_or(too_small)?);
        writer.write(ccmp).map_err(|_| too_small)?;
        writer
            .write(&[p0, p1, p2, p3, p4, p5, e0, e1])
            .map_err(|_| too_small)?;
        Ok(header)
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
        peer_qos: bool,
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

    const MODES: [(bool, LinkProtection); 4] = [
        (false, LinkProtection::Open),
        (true, LinkProtection::Open),
        (false, LinkProtection::Ccmp),
        (true, LinkProtection::Ccmp),
    ];

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
                        let header = template.write(fields, &mut written).unwrap();
                        assert_eq!(template.prefix_len(), length);
                        assert_eq!(written, expected);
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
            template.write(first, &mut output).unwrap();
            let header = template.write(second, &mut output).unwrap();
            assert_eq!(header.address3(), [2, 0, 0, 0, 0, 4]);
            assert_eq!(
                SequenceNumber::from_sequence_control(header.sequence_control()),
                SequenceNumber::new(1).unwrap()
            );
            assert_eq!(
                header.qos().map(|qos| qos.tid().get()),
                peer_qos.then_some(2)
            );
            assert_eq!(
                header.control().protected(),
                protection == LinkProtection::Ccmp
            );
            let mut expected = [0; 64];
            encoded(peer_qos, protection, second, &mut expected);
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn a_short_output_is_refused_before_any_octet_is_written() {
        for (peer_qos, protection) in MODES {
            let template =
                StaDataHeaderTemplate::new(source(), bssid(), peer_qos, protection).unwrap();
            let required = template.prefix_len();
            let mut output = [0xee; 64];
            let fields = StaDataHeaderFields {
                destination: [2, 0, 0, 0, 0, 3],
                sequence_number: SequenceNumber::ZERO,
                priority: WmmUserPriority::UP0,
                ether_type: 0x0800,
            };
            assert_eq!(
                template.write(fields, &mut output[..required - 1]),
                Err(StationFrameError::OutputTooSmall { required })
            );
            assert_eq!(output, [0xee; 64]);
        }
    }

    #[test]
    fn a_template_serves_only_the_association_inputs_it_was_built_from() {
        let template =
            StaDataHeaderTemplate::new(source(), bssid(), true, LinkProtection::Ccmp).unwrap();
        assert!(template.serves(source(), bssid(), true, LinkProtection::Ccmp));
        assert!(!template.serves(source(), bssid(), false, LinkProtection::Ccmp));
        assert!(!template.serves(source(), bssid(), true, LinkProtection::Open));
        let other = OwnAddress::new(IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap());
        assert!(!template.serves(other, bssid(), true, LinkProtection::Ccmp));
        let other = Bssid::new(IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap()).unwrap();
        assert!(!template.serves(source(), other, true, LinkProtection::Ccmp));
    }
}
