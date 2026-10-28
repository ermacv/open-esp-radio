//! Header geometry of a monitor-injected MPDU.
//!
//! A monitor injection carries a caller-encoded, unprotected MPDU without
//! its FCS. This module checks the Frame Control and Sequence Control
//! geometry that the lower-MAC monitor boundary admits.

use core::fmt;

use crate::frame::{FrameControlFields, FrameType};
use crate::sequence::SequenceControl;

/// Frame Control, Duration/ID, three addresses and Sequence Control.
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format) and 9.3.3.2
/// (Management frame format).
const BASE_HEADER_LENGTH: usize = 24;
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Data, subtype 0000).
const DATA_SUBTYPE: u16 = 0;
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Null, subtype 0100).
const NULL_SUBTYPE: u16 = 4;
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1: subtypes 1000 to 1111
/// of the Data type carry a QoS Control field.
const QOS_SUBTYPE_FIRST: u16 = 8;
const QOS_SUBTYPE_LAST: u16 = 15;

/// Supported header geometry of one source-supplied monitor MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionFrameType {
    Management,
    Data,
    NullData,
}

/// A header-geometry-checked, unprotected IEEE 802.11 MPDU without an FCS.
///
/// The constructor name makes FCS ownership explicit: the caller supplies
/// bytes ending at the MPDU body, while a concrete backend must append or ask
/// hardware to append the FCS exactly once.  The source cannot infer whether
/// four arbitrary trailing bytes were intended as an FCS, so that part of the
/// contract is represented by this type rather than guessed from contents.
/// Management bodies remain opaque raw-policy bytes; this type does not claim
/// that subtype-specific information elements or action contents are valid.
///
/// The initial implementation intentionally admits only ordinary 24-byte
/// management headers, non-QoS three-address data and exact Null Data.  It
/// rejects protected, fragmented, ordered, four-address, QoS, control and
/// extension frames before a sequence allocator, key owner or DMA buffer can
/// be borrowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionMpdu<'frame> {
    bytes: &'frame [u8],
    frame_type: MonitorInjectionFrameType,
}

impl<'frame> MonitorInjectionMpdu<'frame> {
    pub fn from_no_fcs(bytes: &'frame [u8]) -> Result<Self, MonitorInjectionFrameError> {
        let too_short = MonitorInjectionFrameError::TooShort {
            required: BASE_HEADER_LENGTH,
            available: bytes.len(),
        };
        let mut reader = oer_wire::Reader::new(bytes);
        let control = FrameControlFields::from_bytes(reader.read().map_err(|_| too_short)?);
        // Duration/ID and Address 1 to Address 3 precede Sequence Control.
        reader.take(20).map_err(|_| too_short)?;
        let sequence = SequenceControl::from_bytes(reader.read().map_err(|_| too_short)?);
        if control.version() != 0 {
            return Err(MonitorInjectionFrameError::UnsupportedProtocolVersion(
                control.version(),
            ));
        }
        if control.protected() {
            return Err(MonitorInjectionFrameError::ProtectedFrame);
        }
        if control.more_fragments() || sequence.fragment() != 0 {
            return Err(MonitorInjectionFrameError::FragmentedFrame);
        }
        if control.order() {
            return Err(MonitorInjectionFrameError::OrderedFrame);
        }
        let frame_type = match control.kind() {
            FrameType::Management => {
                if control.to_ds() || control.from_ds() {
                    return Err(MonitorInjectionFrameError::UnsupportedManagementFlags);
                }
                MonitorInjectionFrameType::Management
            }
            FrameType::Data => {
                if control.to_ds() && control.from_ds() {
                    return Err(MonitorInjectionFrameError::FourAddressData);
                }
                match control.subtype() {
                    DATA_SUBTYPE => MonitorInjectionFrameType::Data,
                    NULL_SUBTYPE if bytes.len() == BASE_HEADER_LENGTH => {
                        MonitorInjectionFrameType::NullData
                    }
                    NULL_SUBTYPE => {
                        return Err(MonitorInjectionFrameError::InvalidNullDataLength {
                            expected: BASE_HEADER_LENGTH,
                            actual: bytes.len(),
                        });
                    }
                    QOS_SUBTYPE_FIRST..=QOS_SUBTYPE_LAST => return Err(MonitorInjectionFrameError::QosData),
                    subtype => {
                        return Err(MonitorInjectionFrameError::UnsupportedDataSubtype(
                            subtype as u8,
                        ));
                    }
                }
            }
            other => {
                return Err(MonitorInjectionFrameError::UnsupportedFrameType(
                    other.encoding(),
                ));
            }
        };
        Ok(Self { bytes, frame_type })
    }

    pub const fn bytes(self) -> &'frame [u8] {
        self.bytes
    }

    pub const fn len(self) -> usize {
        self.bytes.len()
    }

    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub const fn frame_type(self) -> MonitorInjectionFrameType {
        self.frame_type
    }

    pub fn is_group_addressed(self) -> bool {
        self.bytes.get(4).is_some_and(|byte| byte & 1 != 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionFrameError {
    TooShort { required: usize, available: usize },
    UnsupportedProtocolVersion(u8),
    ProtectedFrame,
    FragmentedFrame,
    OrderedFrame,
    UnsupportedManagementFlags,
    FourAddressData,
    QosData,
    UnsupportedDataSubtype(u8),
    InvalidNullDataLength { expected: usize, actual: usize },
    UnsupportedFrameType(u8),
}

impl fmt::Display for MonitorInjectionFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort {
                required,
                available,
            } => write!(
                formatter,
                "monitor injection requires at least {required} no-FCS MPDU bytes, got {available}"
            ),
            Self::UnsupportedProtocolVersion(version) => write!(
                formatter,
                "monitor injection does not support 802.11 protocol version {version}"
            ),
            Self::ProtectedFrame => {
                formatter.write_str("monitor injection does not own a protected/keyed TX path")
            }
            Self::FragmentedFrame => {
                formatter.write_str("monitor injection does not own fragment sequencing")
            }
            Self::OrderedFrame => {
                formatter.write_str("monitor injection does not own ordered/HT-Control geometry")
            }
            Self::UnsupportedManagementFlags => formatter
                .write_str("monitor management injection requires both DS direction bits clear"),
            Self::FourAddressData => {
                formatter.write_str("monitor injection does not own four-address data geometry")
            }
            Self::QosData => {
                formatter.write_str("monitor injection does not own QoS data geometry")
            }
            Self::UnsupportedDataSubtype(subtype) => write!(
                formatter,
                "monitor injection does not support data subtype {subtype}"
            ),
            Self::InvalidNullDataLength { expected, actual } => write!(
                formatter,
                "monitor Null Data injection requires exactly {expected} bytes, got {actual}"
            ),
            Self::UnsupportedFrameType(frame_type) => write!(
                formatter,
                "monitor injection does not support frame type {frame_type}"
            ),
        }
    }
}

impl core::error::Error for MonitorInjectionFrameError {}


#[cfg(test)]
mod tests;
