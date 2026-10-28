//! Complete Link Layer PDUs crossing the radio boundary.

/// Why bytes do not form the required PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PduError {
    /// Fewer bytes than the two-byte header and the required payload.
    TooShort,
    /// More payload than the PDU admits.
    TooLong,
    /// The header length byte disagrees with the supplied payload.
    LengthMismatch,
    /// The value is outside its field.
    OutOfRange,
}

/// Maximum payload of a legacy advertising PDU.
pub(crate) const ADVERTISING_PAYLOAD_MAX: usize = 37;
/// Advertiser address that starts every legacy advertising payload.
pub(crate) const ADVERTISER_ADDRESS_BYTES: usize = 6;
/// Maximum payload of a data channel PDU on air: 251 octets of data and an
/// encrypted packet's 4-octet MIC.
pub(crate) const DATA_PAYLOAD_MAX: usize = 255;

/// One encoded legacy advertising PDU: header, length and payload starting
/// with the advertiser address. Protocol validity stays with the caller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AdvertisingPdu<'pdu> {
    bytes: &'pdu [u8],
    // Retain the admitted octet so header access needs no slice indexing.
    header: u8,
}

impl<'pdu> AdvertisingPdu<'pdu> {
    /// Check the extent of one encoded PDU.
    pub const fn new(bytes: &'pdu [u8]) -> Result<Self, PduError> {
        let [header, length, payload @ ..] = bytes else {
            return Err(PduError::TooShort);
        };
        if payload.len() < ADVERTISER_ADDRESS_BYTES {
            return Err(PduError::TooShort);
        }
        if payload.len() > ADVERTISING_PAYLOAD_MAX {
            return Err(PduError::TooLong);
        }
        if *length as usize != payload.len() {
            return Err(PduError::LengthMismatch);
        }
        Ok(Self {
            bytes,
            header: *header,
        })
    }

    /// The complete encoded PDU.
    pub const fn bytes(self) -> &'pdu [u8] {
        self.bytes
    }

    /// The two-byte header's first octet.
    pub const fn header(self) -> u8 {
        self.header
    }
}

/// The LLID of a data channel PDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DataPduKind {
    /// Continuation fragment of an L2CAP message, or an empty PDU.
    Continuation,
    /// Start of an L2CAP message, or a complete one.
    Start,
    /// Link Layer control PDU.
    Control,
}

/// One data channel PDU queued on a connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DataPdu<'pdu> {
    kind: DataPduKind,
    payload: &'pdu [u8],
}

impl<'pdu> DataPdu<'pdu> {
    /// Check the payload length.
    pub const fn new(kind: DataPduKind, payload: &'pdu [u8]) -> Result<Self, PduError> {
        if payload.len() > DATA_PAYLOAD_MAX {
            return Err(PduError::TooLong);
        }
        Ok(Self { kind, payload })
    }

    /// The LLID.
    pub const fn kind(self) -> DataPduKind {
        self.kind
    }

    /// The payload after the header.
    pub const fn payload(self) -> &'pdu [u8] {
        self.payload
    }
}

/// One of the eight standard LE Test packet payload types.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TestPayloadType(u8);

impl TestPayloadType {
    /// Validate the LE Test PDU type field.
    pub const fn new(value: u8) -> Result<Self, PduError> {
        if value < 8 {
            Ok(Self(value))
        } else {
            Err(PduError::OutOfRange)
        }
    }

    /// The type field.
    pub const fn value(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::arithmetic_side_effects)]
mod tests {
    use super::{AdvertisingPdu, DataPdu, DataPduKind, PduError, TestPayloadType};

    #[test]
    fn advertising_pdus_carry_the_advertiser_and_a_consistent_length() {
        let pdu = [0x02, 6, 1, 2, 3, 4, 5, 6];
        assert_eq!(
            AdvertisingPdu::new(&pdu).map(AdvertisingPdu::bytes),
            Ok(&pdu[..])
        );
        assert_eq!(AdvertisingPdu::new(&pdu[..7]), Err(PduError::TooShort));
        assert_eq!(
            AdvertisingPdu::new(&[0x02, 7, 1, 2, 3, 4, 5, 6]),
            Err(PduError::LengthMismatch)
        );
        let mut long = [0u8; 40];
        long[1] = 38;
        assert_eq!(AdvertisingPdu::new(&long), Err(PduError::TooLong));
    }

    #[test]
    fn advertising_extent_checks_preserve_error_precedence_and_header() {
        let mut bytes = [0; 40];
        // Cover every length octet, including values far beyond legacy PDUs,
        // and both sides of the admitted extent. No input needs length addition.
        for length in u8::MIN..=u8::MAX {
            bytes[0] = length;
            bytes[1] = length;
            for extent in 0..=bytes.len() {
                let input = &bytes[..extent];
                let result = AdvertisingPdu::new(input);
                if extent < 8 {
                    assert_eq!(result, Err(PduError::TooShort));
                } else if extent > 39 {
                    assert_eq!(result, Err(PduError::TooLong));
                } else if usize::from(length) != extent - 2 {
                    assert_eq!(result, Err(PduError::LengthMismatch));
                } else {
                    let pdu = result.unwrap();
                    assert_eq!(pdu.header(), length);
                    assert_eq!(pdu.bytes(), input);
                }
            }
        }
        // Every header octet remains opaque to extent validation.
        for header in u8::MIN..=u8::MAX {
            let bytes = [header, 6, 1, 2, 3, 4, 5, 6];
            assert_eq!(AdvertisingPdu::new(&bytes).unwrap().header(), header);
        }
    }

    #[test]
    fn data_payloads_and_test_types_are_bounded() {
        assert!(DataPdu::new(DataPduKind::Control, &[0; 255]).is_ok());
        assert_eq!(
            DataPdu::new(DataPduKind::Start, &[0; 256]),
            Err(PduError::TooLong)
        );
        assert_eq!(TestPayloadType::new(7).map(TestPayloadType::value), Ok(7));
        assert_eq!(TestPayloadType::new(8), Err(PduError::OutOfRange));
    }
}
