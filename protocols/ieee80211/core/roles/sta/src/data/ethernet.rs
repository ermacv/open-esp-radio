//! Owned frames at the network queue boundary, held as MSDU fields.

use oer_ieee80211_wire::data::{MsduHeader, MsduParts};
use oer_ieee80211_wire::qos::WmmUserPriority;

/// Maximum Ethernet-II payload stored by one network queue slot.
pub const MAX_ETHERNET_PAYLOAD_LEN: usize = 1500;

/// An Ethernet payload exceeds [`MAX_ETHERNET_PAYLOAD_LEN`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTooLong {
    /// The supplied payload length; no bytes were accepted or clipped.
    pub payload_length: usize,
}

/// One frame's addresses and EtherType and a bounded, owned payload.
///
/// The frame holds fields, not a header layout: the network side's adapter
/// reads and writes its Ethernet header through its own codec. Construction
/// refuses payloads above [`MAX_ETHERNET_PAYLOAD_LEN`]; it never clips
/// bytes.
///
/// A network adapter constructs a frame from its checked Ethernet parts:
///
/// ```
/// use oer_ieee80211_wire::data::MsduParts;
/// use oer_ieee80211_core_roles_sta::data::EthernetFrame;
/// let parts = MsduParts {
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
/// use oer_ieee80211_core_roles_sta::data::EthernetFrame;
/// let frame = EthernetFrame::new(&[0; 14]);
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_core_roles_sta::data::EthernetFrame;
/// fn bytes(frame: &EthernetFrame) { let _bytes = frame.bytes(); }
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_core_roles_sta::data::EthernetFrame;
/// fn copy(frame: &EthernetFrame, output: &mut [u8]) {
///     let _copied = frame.copy_to(output);
/// }
/// ```
#[derive(Debug)]
pub struct EthernetFrame {
    header: MsduHeader,
    payload: heapless::Vec<u8, MAX_ETHERNET_PAYLOAD_LEN>,
}

impl EthernetFrame {
    /// Own the complete frame, refusing an oversized payload before copying.
    pub fn from_parts(parts: MsduParts<'_>) -> Result<Self, FrameTooLong> {
        let payload = heapless::Vec::from_slice(parts.payload).map_err(|_| FrameTooLong {
            payload_length: parts.payload.len(),
        })?;
        Ok(Self {
            header: parts.header(),
            payload,
        })
    }

    /// Borrow the header fields and payload for the MAC or adapter.
    pub fn parts(&self) -> MsduParts<'_> {
        let MsduHeader {
            destination,
            source,
            ether_type,
        } = self.header;
        MsduParts {
            destination,
            source,
            ether_type,
            payload: &self.payload,
        }
    }
}

impl oer_network_port::EthernetFrame for EthernetFrame {
    fn view(
        &self,
    ) -> Result<oer_network_port::EthernetFrameView<'_>, oer_network_port::FrameLengthError> {
        let parts = self.parts();
        Ok(oer_network_port::EthernetFrameView {
            destination: parts.destination,
            source: parts.source,
            ether_type: parts.ether_type,
            payload: parts.payload,
        })
    }
}

impl TxMsdu {
    /// Copy a neutral network frame into this station's checked queue owner,
    /// with the user priority its classification requests.
    pub fn from_network(
        frame: oer_network_port::EthernetFrameView<'_>,
        priority: WmmUserPriority,
    ) -> Result<Self, FrameTooLong> {
        Ok(Self {
            priority,
            frame: EthernetFrame::from_parts(MsduParts {
                destination: frame.destination,
                source: frame.source,
                ether_type: frame.ether_type,
                payload: frame.payload,
            })?,
        })
    }
}

/// One checked, owned Ethernet frame with its user priority, as the network
/// stack hands it to the station's data path.
#[derive(Debug)]
pub struct TxMsdu {
    pub priority: WmmUserPriority,
    pub frame: EthernetFrame,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_maximum_payload_are_kept_without_truncation() {
        let parts = MsduParts {
            destination: [0xab; 6],
            source: [0xcd; 6],
            ether_type: 0xabcd,
            payload: &[0xab; MAX_ETHERNET_PAYLOAD_LEN],
        };
        let frame = EthernetFrame::from_parts(parts).unwrap();
        assert_eq!(frame.parts(), parts);
        assert_eq!(
            EthernetFrame::from_parts(MsduParts {
                payload: &[0; MAX_ETHERNET_PAYLOAD_LEN + 1],
                ..parts
            })
            .unwrap_err(),
            FrameTooLong {
                payload_length: MAX_ETHERNET_PAYLOAD_LEN + 1
            }
        );
    }
}
