//! Owned association RSN/RSNXE elements.
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

use super::*;
use oer_ieee80211_wire::element::Element;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRsnIe<const N: usize = RSN_IE_CAPACITY> {
    bytes: heapless::Vec<u8, N>,
}

impl<const N: usize> OwnedRsnIe<N> {
    pub fn try_copy(bytes: &[u8]) -> Result<Self, RsnFrameError> {
        if !Element::exact(bytes).is_some_and(|element| element.id == RSN_ELEMENT_ID) {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        let bytes =
            heapless::Vec::from_slice(bytes).map_err(|_| RsnFrameError::CapacityExceeded)?;
        Ok(Self { bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

/// Exact RSN IE plus optional RSNXE emitted by the STA association request.
///
/// WPA2 message 2 must echo this complete byte sequence. Keeping it separate
/// from [`OwnedRsnIe`] preserves the stricter single-element invariant used
/// when validating message 3 key data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedAssociationSecurityIes<const N: usize = RSN_ASSOC_SECURITY_IES_CAPACITY> {
    bytes: heapless::Vec<u8, N>,
}

impl<const N: usize> OwnedAssociationSecurityIes<N> {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn invalidate_for_test(&mut self) {
        self.bytes.clear();
    }

    /// Copy the exact RSN IE plus optional RSNXE emitted by Association.
    pub fn try_copy_bytes(bytes: &[u8]) -> Result<Self, RsnFrameError> {
        let Some((rsn, rsnxe)) = Element::split(bytes) else {
            return Err(RsnFrameError::InvalidRsnIe);
        };
        if rsn.id != RSN_ELEMENT_ID {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        let rsn = OwnedRsnIe::<RSN_IE_CAPACITY>::try_copy(rsn.bytes)?;
        Self::try_copy(&rsn, rsnxe)
    }

    pub fn try_copy<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        rsnxe: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if !rsnxe.is_empty()
            && !Element::exact(rsnxe).is_some_and(|element| element.id == RSNXE_ELEMENT_ID)
        {
            return Err(RsnFrameError::InvalidRsnxe);
        }
        let len = rsn_ie
            .as_bytes()
            .len()
            .checked_add(rsnxe.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N {
            return Err(RsnFrameError::CapacityExceeded);
        }
        let mut bytes = heapless::Vec::from_slice(rsn_ie.as_bytes())
            .map_err(|_| RsnFrameError::CapacityExceeded)?;
        bytes
            .extend_from_slice(rsnxe)
            .map_err(|_| RsnFrameError::CapacityExceeded)?;
        Ok(Self { bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// The RSN element and the RSNXE after it.
    fn split(&self) -> Result<(&[u8], &[u8]), RsnFrameError> {
        Element::split(self.as_bytes())
            .map(|(rsn, rsnxe)| (rsn.bytes, rsnxe))
            .ok_or(RsnFrameError::InvalidRsnIe)
    }
    pub fn rsn_ie(&self) -> Result<&[u8], RsnFrameError> {
        Ok(self.split()?.0)
    }
    pub fn rsnxe(&self) -> Result<&[u8], RsnFrameError> {
        Ok(self.split()?.1)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects
    )]
    use super::*;

    #[test]
    fn damaged_association_envelopes_return_errors_for_both_views() {
        for bytes in [&[][..], &[RSN_ELEMENT_ID][..], &[RSN_ELEMENT_ID, 255][..]] {
            let owned = OwnedAssociationSecurityIes::<4> {
                bytes: heapless::Vec::from_slice(bytes).unwrap(),
            };
            assert_eq!(owned.rsn_ie(), Err(RsnFrameError::InvalidRsnIe));
            assert_eq!(owned.rsnxe(), Err(RsnFrameError::InvalidRsnIe));
        }
    }
}
