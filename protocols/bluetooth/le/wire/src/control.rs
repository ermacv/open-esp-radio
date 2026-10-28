//! Data and LL Control PDU envelopes and fixed control bodies (Core Vol 6 Part B 2.4).

use crate::connection::{LeConnectionTiming, LeDataChannelMap};
use oer_wire::PrefixLength;

/// Controller identity supplied by the product; independent of the radio vendor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeVersionInformation([u8; 5]);

impl LeVersionInformation {
    pub const fn new(version: u8, company_identifier: u16, subversion: u16) -> Self {
        let company = company_identifier.to_le_bytes();
        let subversion = subversion.to_le_bytes();
        Self([
            version,
            company[0],
            company[1],
            subversion[0],
            subversion[1],
        ])
    }
    pub const fn version(self) -> u8 {
        self.0[0]
    }
    pub const fn company_identifier(self) -> u16 {
        u16::from_le_bytes([self.0[1], self.0[2]])
    }
    pub const fn subversion(self) -> u16 {
        u16::from_le_bytes([self.0[3], self.0[4]])
    }
}

/// Payload of one response, excluding the hardware-owned data-channel header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControlResponse {
    bytes: [u8; 9],
    len: PrefixLength<9>,
}

impl LeControlResponse {
    pub fn as_bytes(&self) -> &[u8] {
        self.len.borrow(&self.bytes)
    }
}

/// One accepted unencrypted LE-U payload borrowed from its completed RX batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePeripheralDataFragment<'a> {
    payload: &'a [u8],
    continuing: bool,
}

/// Validated Channel Map Update carried by `LL_CHANNEL_MAP_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeChannelMapUpdate {
    channel_map: LeDataChannelMap,
    instant: u16,
}

/// Validated Connection Update carried by `LL_CONNECTION_UPDATE_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionUpdate {
    timing: LeConnectionTiming,
    instant: u16,
}

impl LeConnectionUpdate {
    pub const fn timing(self) -> LeConnectionTiming {
        self.timing
    }

    pub const fn instant(self) -> u16 {
        self.instant
    }
}

impl LeChannelMapUpdate {
    pub const fn channel_map(self) -> LeDataChannelMap {
        self.channel_map
    }

    pub const fn instant(self) -> u16 {
        self.instant
    }
}

impl<'a> LePeripheralDataFragment<'a> {
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }

    pub const fn is_continuing(self) -> bool {
        self.continuing
    }
}

pub enum LePeripheralPdu<'a> {
    Data(LePeripheralDataFragment<'a>),
    Control(&'a [u8]),
}

pub fn decode_peripheral_pdu(pdu: &[u8]) -> Result<LePeripheralPdu<'_>, LePduDecodeError> {
    use LePduDecodeError as Error;
    let [header, length, payload @ ..] = pdu else {
        return Err(Error::MalformedPdu);
    };
    if payload.len() != usize::from(*length) {
        return Err(Error::MalformedPdu);
    }
    // CTE is outside this profile. RFU bits are ignored as required by LL.
    if *header & 0x20 != 0 {
        return Err(Error::UnsupportedHeader);
    }
    match *header & 3 {
        llid @ (1 | 2) => Ok(LePeripheralPdu::Data(LePeripheralDataFragment {
            payload,
            continuing: llid == 1,
        })),
        3 if !payload.is_empty() => Ok(LePeripheralPdu::Control(payload)),
        _ => Err(Error::MalformedPdu),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePduDecodeError {
    MalformedPdu,
    UnsupportedHeader,
}

impl LeVersionInformation {
    pub const fn from_wire_bytes(bytes: [u8; 5]) -> Self {
        Self(bytes)
    }
    pub const fn wire_bytes(self) -> [u8; 5] {
        self.0
    }
}
impl LeControlResponse {
    pub const fn from_length_pdu(bytes: [u8; 9]) -> Self {
        Self {
            bytes,
            len: PrefixLength::constant::<9>(),
        }
    }
    pub const fn features(opcode: u8, features: [u8; 8]) -> Self {
        let [a, b, c, d, e, f, g, h] = features;
        Self::from_length_pdu([opcode, a, b, c, d, e, f, g, h])
    }
    pub const fn version(version: LeVersionInformation) -> Self {
        let [a, b, c, d, e] = version.wire_bytes();
        Self {
            bytes: [0x0c, a, b, c, d, e, 0, 0, 0],
            len: PrefixLength::constant::<6>(),
        }
    }
    pub const fn ping_response() -> Self {
        Self {
            bytes: [0x13, 0, 0, 0, 0, 0, 0, 0, 0],
            len: PrefixLength::constant::<1>(),
        }
    }
    pub const fn unknown(opcode: u8) -> Self {
        Self {
            bytes: [0x07, opcode, 0, 0, 0, 0, 0, 0, 0],
            len: PrefixLength::constant::<2>(),
        }
    }
    pub const fn terminate(reason: u8) -> Self {
        Self {
            bytes: [0x02, reason, 0, 0, 0, 0, 0, 0, 0],
            len: PrefixLength::constant::<2>(),
        }
    }
}
impl LeChannelMapUpdate {
    pub fn decode(payload: &[u8]) -> Result<Self, LePduDecodeError> {
        let [0x01, a, b, c, d, e, lo, hi] = *payload else {
            return Err(LePduDecodeError::MalformedPdu);
        };
        Ok(Self {
            channel_map: LeDataChannelMap::new([a, b, c, d, e])
                .map_err(|_| LePduDecodeError::MalformedPdu)?,
            instant: u16::from_le_bytes([lo, hi]),
        })
    }
}
impl LeConnectionUpdate {
    pub fn decode(payload: &[u8]) -> Result<Self, LePduDecodeError> {
        let [0x00, size, ol, oh, il, ih, ll, lh, tl, th, el, eh] = *payload else {
            return Err(LePduDecodeError::MalformedPdu);
        };
        let timing = LeConnectionTiming::new(
            size,
            u16::from_le_bytes([ol, oh]),
            u16::from_le_bytes([il, ih]),
            u16::from_le_bytes([ll, lh]),
            u16::from_le_bytes([tl, th]),
        )
        .map_err(|_| LePduDecodeError::MalformedPdu)?;
        Ok(Self {
            timing,
            instant: u16::from_le_bytes([el, eh]),
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    #[test]
    fn data_envelope_retains_error_precedence_and_fragment_kind() {
        assert!(matches!(
            decode_peripheral_pdu(&[0x23, 1]),
            Err(LePduDecodeError::MalformedPdu)
        ));
        assert!(matches!(
            decode_peripheral_pdu(&[0x23, 1, 0x12]),
            Err(LePduDecodeError::UnsupportedHeader)
        ));
        for header in 0..=u8::MAX {
            let pdu = [header, 1, 0x12];
            match (header & 0x20, header & 3) {
                (0, 1 | 2) => {
                    let LePeripheralPdu::Data(fragment) = decode_peripheral_pdu(&pdu).unwrap()
                    else {
                        panic!("data fragment");
                    };
                    assert_eq!(fragment.payload(), [0x12]);
                    assert_eq!(fragment.is_continuing(), header & 3 == 1);
                }
                (0, 3) => assert!(matches!(
                    decode_peripheral_pdu(&pdu),
                    Ok(LePeripheralPdu::Control([0x12]))
                )),
                (0, _) => assert!(matches!(
                    decode_peripheral_pdu(&pdu),
                    Err(LePduDecodeError::MalformedPdu)
                )),
                _ => assert!(matches!(
                    decode_peripheral_pdu(&pdu),
                    Err(LePduDecodeError::UnsupportedHeader)
                )),
            }
        }
    }

    #[test]
    fn control_builders_preserve_the_exact_payload_bytes() {
        let version = LeVersionInformation::new(0x0e, 0x1234, 0x5678);
        assert_eq!(
            LeControlResponse::version(version).as_bytes(),
            [0x0c, 0x0e, 0x34, 0x12, 0x78, 0x56]
        );
        assert_eq!(
            LeControlResponse::features(0x09, [1, 2, 3, 4, 5, 6, 7, 8]).as_bytes(),
            [0x09, 1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(LeControlResponse::ping_response().as_bytes(), [0x13]);
        assert_eq!(LeControlResponse::unknown(0x42).as_bytes(), [0x07, 0x42]);
        assert_eq!(LeControlResponse::terminate(0x16).as_bytes(), [0x02, 0x16]);
    }

    #[test]
    fn updates_validate_exact_bodies_and_preserve_instants() {
        let map = [0x01, 0xff, 0xff, 0xff, 0xff, 0x1f, 0xfe, 0xff];
        let update = LeChannelMapUpdate::decode(&map).unwrap();
        assert_eq!(update.channel_map(), LeDataChannelMap::all());
        assert_eq!(update.instant(), 0xfffe);
        for length in 0..map.len() {
            assert_eq!(
                LeChannelMapUpdate::decode(&map[..length]),
                Err(LePduDecodeError::MalformedPdu)
            );
        }
        let timing = [0x00, 2, 1, 0, 24, 0, 0, 0, 200, 0, 0xff, 0xff];
        let update = LeConnectionUpdate::decode(&timing).unwrap();
        assert_eq!(update.timing().interval_units(), 24);
        assert_eq!(update.instant(), 0xffff);
        for length in 0..timing.len() {
            assert_eq!(
                LeConnectionUpdate::decode(&timing[..length]),
                Err(LePduDecodeError::MalformedPdu)
            );
        }
    }
}
