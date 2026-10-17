//! Checked Ethernet-II storage at the network queue boundary.

use oer_ieee80211_mac::data::{ETHERNET_HEADER_LEN, EthernetFrameParts};

/// Maximum Ethernet-II payload stored by one network queue slot.
pub const MAX_ETHERNET_PAYLOAD_LEN: usize = 1500;

/// An Ethernet payload exceeds [`MAX_ETHERNET_PAYLOAD_LEN`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTooLong {
    /// The supplied payload length; no bytes were accepted or clipped.
    pub payload_length: usize,
}

/// One complete Ethernet-II header and a bounded, owned payload.
///
/// No independent length can disagree with the storage. Construction accepts
/// a complete header as parts and refuses payloads above
/// [`MAX_ETHERNET_PAYLOAD_LEN`]; it never clips bytes.
///
/// A network adapter constructs a frame from its checked Ethernet parts:
///
/// ```
/// use oer_ieee80211_mac::data::EthernetFrameParts;
/// use oer_ieee80211_runtime::station_service::EthernetFrame;
/// let parts = EthernetFrameParts {
///     destination: [0xff; 6], source: [2, 0, 0, 0, 0, 1],
///     ether_type: 0x0800, payload: &[1, 2, 3],
/// };
/// let frame = EthernetFrame::from_parts(parts).unwrap();
/// assert_eq!(frame.parts(), parts);
/// ```
///
/// Test fixture parsing and serialization are absent from the product API:
///
/// ```compile_fail,E0599
/// use oer_ieee80211_runtime::station_service::EthernetFrame;
/// let frame = EthernetFrame::new(&[0; 14]);
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_runtime::station_service::EthernetFrame;
/// fn bytes(frame: &EthernetFrame) { let _bytes = frame.bytes(); }
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_runtime::station_service::EthernetFrame;
/// fn copy(frame: &EthernetFrame, output: &mut [u8]) {
///     let _copied = frame.copy_to(output);
/// }
/// ```
#[derive(Debug)]
pub struct EthernetFrame {
    header: [u8; ETHERNET_HEADER_LEN],
    payload: heapless::Vec<u8, MAX_ETHERNET_PAYLOAD_LEN>,
}

impl EthernetFrame {
    /// Own the complete frame, refusing an oversized payload before copying.
    pub fn from_parts(parts: EthernetFrameParts<'_>) -> Result<Self, FrameTooLong> {
        let payload = heapless::Vec::from_slice(parts.payload).map_err(|_| FrameTooLong {
            payload_length: parts.payload.len(),
        })?;
        let [d0, d1, d2, d3, d4, d5] = parts.destination;
        let [s0, s1, s2, s3, s4, s5] = parts.source;
        let [e0, e1] = parts.ether_type.to_be_bytes();
        Ok(Self {
            header: [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1],
            payload,
        })
    }

    /// Borrow the complete header fields and payload for the MAC or adapter.
    pub fn parts(&self) -> EthernetFrameParts<'_> {
        let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = self.header;
        EthernetFrameParts {
            destination: [d0, d1, d2, d3, d4, d5],
            source: [s0, s1, s2, s3, s4, s5],
            ether_type: u16::from_be_bytes([e0, e1]),
            payload: &self.payload,
        }
    }

    /// The complete Ethernet-II length, including the fixed header.
    pub fn length(&self) -> usize {
        // The payload is bounded by MAX_ETHERNET_PAYLOAD_LEN.
        self.parts().length()
    }
}

/// A test's serialization destination cannot hold the complete frame.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CopyError {
    available: usize,
    required: usize,
}

#[cfg(test)]
impl EthernetFrame {
    pub(super) fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.header.iter().chain(self.payload.iter()).copied()
    }

    /// Validate the whole destination before writing any bytes.
    fn copy_to(&self, output: &mut [u8]) -> Result<usize, CopyError> {
        let available = output.len();
        let required = self.length();
        let (header, rest) = output
            .split_first_chunk_mut::<ETHERNET_HEADER_LEN>()
            .ok_or(CopyError {
                available,
                required,
            })?;
        let (payload, _) = rest
            .split_at_mut_checked(self.payload.len())
            .ok_or(CopyError {
                available,
                required,
            })?;
        *header = self.header;
        for (to, from) in payload.iter_mut().zip(&self.payload) {
            *to = *from;
        }
        Ok(required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_maximum_payload_are_proved_once_without_truncation() {
        const FRAME_LEN: usize = ETHERNET_HEADER_LEN + MAX_ETHERNET_PAYLOAD_LEN;
        let bytes = [0xab; FRAME_LEN];
        let parts = EthernetFrameParts {
            destination: [0xab; 6],
            source: [0xab; 6],
            ether_type: 0xabab,
            payload: &[0xab; MAX_ETHERNET_PAYLOAD_LEN],
        };
        let frame = EthernetFrame::from_parts(parts).unwrap();
        assert_eq!(frame.parts(), parts);
        assert_eq!(frame.bytes().collect::<std::vec::Vec<_>>(), bytes);
        assert_eq!(
            EthernetFrame::from_parts(EthernetFrameParts {
                payload: &[0; MAX_ETHERNET_PAYLOAD_LEN + 1],
                ..parts
            })
            .unwrap_err(),
            FrameTooLong {
                payload_length: MAX_ETHERNET_PAYLOAD_LEN + 1
            }
        );
        let mut short = [0x55; FRAME_LEN - 1];
        assert_eq!(
            frame.copy_to(&mut short),
            Err(CopyError {
                available: FRAME_LEN - 1,
                required: FRAME_LEN
            })
        );
        assert_eq!(short, [0x55; FRAME_LEN - 1]);
        let mut output = [0; FRAME_LEN];
        assert_eq!(frame.copy_to(&mut output), Ok(FRAME_LEN));
        assert_eq!(output, bytes);
    }
}
