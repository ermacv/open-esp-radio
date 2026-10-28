//! LE 1M packet air time and inter-frame spacing (Bluetooth Core 6.0, Vol 6).

use crate::LE_DEVICE_ADDRESS_LENGTH;
use crate::advertising_header::ADVERTISING_HEADER_LENGTH;

/// Inter Frame Space between consecutive packets on one channel index
/// (Core Vol 6, Part B, 4.1.1).
pub const T_IFS_MICROS: u32 = 150;

/// Preamble octets of an LE 1M packet (Core Vol 6, Part B, 2.1.1).
pub const LE_1M_PREAMBLE_OCTETS: u32 = 1;
/// Access Address octets (Core Vol 6, Part B, 2.1.2).
pub const ACCESS_ADDRESS_OCTETS: u32 = 4;
/// CRC octets (Core Vol 6, Part B, 2.1.4).
pub const CRC_OCTETS: u32 = 3;
/// Octets of an LE 1M packet around its PDU: preamble, Access Address and CRC
/// (Core Vol 6, Part B, 2.1).
pub const LE_1M_PACKET_OVERHEAD_OCTETS: u32 =
    LE_1M_PREAMBLE_OCTETS + ACCESS_ADDRESS_OCTETS + CRC_OCTETS;
/// Microseconds per octet on LE 1M: one bit per symbol at 1 Msym/s
/// (Core Vol 6, Part A, 3.1).
pub const LE_1M_MICROS_PER_OCTET: u32 = 8;

/// `SCAN_REQ` payload octets: ScanA and AdvA (Core Vol 6, Part B, 2.3.2.1).
pub const SCAN_REQ_PAYLOAD_BYTES: usize = 2 * LE_DEVICE_ADDRESS_LENGTH;
/// Complete on-air duration of a `SCAN_REQ` on LE 1M: its packet overhead,
/// header and payload.
pub const LEGACY_SCAN_REQ_LE_1M_AIRTIME_MICROS: u32 = (LE_1M_PACKET_OVERHEAD_OCTETS
    + (ADVERTISING_HEADER_LENGTH + SCAN_REQ_PAYLOAD_BYTES) as u32)
    * LE_1M_MICROS_PER_OCTET;

/// The length of a legacy advertising-channel PDU, header included: from
/// the two header bytes alone up to the 37 payload bytes of a legacy PDU
/// (Core Vol 6, Part B, 2.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPduLength(u8);

impl LegacyPduLength {
    /// The shortest legacy PDU: its header only.
    pub const MIN: Self = Self(2);
    /// The longest legacy PDU: its header and 37 payload bytes.
    pub const MAX: Self = Self(39);

    /// `length` bytes, or `None` outside `2..=39`.
    pub const fn new(length: usize) -> Option<Self> {
        if length >= Self::MIN.0 as usize && length <= Self::MAX.0 as usize {
            Some(Self(length as u8))
        } else {
            None
        }
    }

    /// The length in bytes.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

#[cfg(test)]
mod tests;
