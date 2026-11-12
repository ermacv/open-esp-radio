//! Owned frames at the network queue boundary, held as MSDU fields.

use oer_protocols_ieee80211_wire::data::{MsduHeader, MsduParts};

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
/// use oer_protocols_ieee80211_wire::data::MsduParts;
/// use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
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
/// use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
/// let frame = EthernetFrame::new(&[0; 14]);
/// ```
///
/// ```compile_fail,E0599
/// use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
/// fn bytes(frame: &EthernetFrame) { let _bytes = frame.bytes(); }
/// ```
///
/// ```compile_fail,E0599
/// use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
/// fn copy(frame: &EthernetFrame, output: &mut [u8]) {
///     let _copied = frame.copy_to(output);
/// }
/// ```
#[derive(Clone, Debug)]
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

    /// A frame of no payload, to be written in place
    /// ([`Self::set_parts`]).
    pub const fn empty() -> Self {
        Self {
            header: MsduHeader {
                destination: [0; 6],
                source: [0; 6],
                ether_type: 0,
            },
            payload: heapless::Vec::new(),
        }
    }

    /// Hold `parts` in place of what the frame held: one copy of the
    /// payload into the frame's own storage.
    ///
    /// # Errors
    /// [`FrameTooLong`]: the frame is left empty, nothing clipped.
    pub fn set_parts(&mut self, parts: MsduParts<'_>) -> Result<(), FrameTooLong> {
        self.payload.clear();
        self.header = parts.header();
        self.payload
            .extend_from_slice(parts.payload)
            .map_err(|_| FrameTooLong {
                payload_length: parts.payload.len(),
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

impl oer_protocols_network_port::EthernetFrame for EthernetFrame {
    fn view(
        &mut self,
    ) -> Result<
        oer_protocols_network_port::EthernetFrameView<'_>,
        oer_protocols_network_port::FrameLengthError,
    > {
        let parts = self.parts();
        Ok(oer_protocols_network_port::EthernetFrameView {
            destination: parts.destination,
            source: parts.source,
            ether_type: parts.ether_type,
            payload: parts.payload,
        })
    }
}

/// One MSDU the station delivers to the network: its fields, and where its
/// payload lies in the MPDU it came in.
///
/// `payload` indexes the delivered MPDU's bytes and always lies within them;
/// a receive owner that keeps the MPDU can hand the payload on in place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MsduDelivery<'frame> {
    /// The MSDU's addresses, EtherType and payload.
    pub parts: MsduParts<'frame>,
    /// The payload's range in the MPDU.
    pub payload: core::ops::Range<usize>,
}

/// A delivered MSDU's fields and where its payload lies in its MPDU,
/// without borrowing the MPDU ([`MsduDelivery::plan`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MsduPlan {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    /// The payload's range in the MPDU.
    pub payload: core::ops::Range<usize>,
}

/// Why a planned MSDU could not be copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedCopyError {
    /// The payload exceeds [`MAX_ETHERNET_PAYLOAD_LEN`].
    TooLong(FrameTooLong),
    /// The payload's range is not inside the given MPDU.
    OutsideFrame,
}

impl EthernetFrame {
    /// Own the MSDU `plan` names in `frame`, its MPDU.
    pub fn copy_planned(frame: &[u8], plan: &MsduPlan) -> Result<Self, PlannedCopyError> {
        let payload = frame
            .get(plan.payload.clone())
            .ok_or(PlannedCopyError::OutsideFrame)?;
        Self::from_parts(MsduParts {
            destination: plan.destination,
            source: plan.source,
            ether_type: plan.ether_type,
            payload,
        })
        .map_err(PlannedCopyError::TooLong)
    }
}

impl<'frame> MsduDelivery<'frame> {
    /// Its fields and payload range, without the borrow.
    pub fn plan(&self) -> MsduPlan {
        MsduPlan {
            destination: self.parts.destination,
            source: self.parts.source,
            ether_type: self.parts.ether_type,
            payload: self.payload.clone(),
        }
    }

    /// The delivery of `parts`, whose payload must be a slice of `frame`;
    /// `None` if it is not.
    pub fn locate(frame: &'frame [u8], parts: MsduParts<'frame>) -> Option<Self> {
        let start = parts
            .payload
            .as_ptr()
            .addr()
            .checked_sub(frame.as_ptr().addr())?;
        let end = start
            .checked_add(parts.payload.len())
            .filter(|end| *end <= frame.len())?;
        Some(Self {
            parts,
            payload: start..end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delivery_locates_its_payload_in_the_mpdu_only() {
        let frame = [0_u8, 1, 2, 3, 4, 5, 6, 7];
        let parts = |payload| MsduParts {
            destination: [1; 6],
            source: [2; 6],
            ether_type: 0x0800,
            payload,
        };
        let delivery = MsduDelivery::locate(&frame, parts(&frame[3..6])).unwrap();
        assert_eq!(delivery.payload, 3..6);
        assert_eq!(&frame[delivery.payload.clone()], delivery.parts.payload);
        let plan = delivery.plan();
        let copied = EthernetFrame::copy_planned(&frame, &plan).unwrap();
        assert_eq!(copied.parts(), delivery.parts);
        assert_eq!(
            EthernetFrame::copy_planned(&frame[..4], &plan).unwrap_err(),
            PlannedCopyError::OutsideFrame
        );
        let other = [0_u8; 4];
        assert_eq!(MsduDelivery::locate(&frame, parts(&other)), None);
        assert_eq!(MsduDelivery::locate(&frame[..4], parts(&frame[3..6])), None);
    }

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
