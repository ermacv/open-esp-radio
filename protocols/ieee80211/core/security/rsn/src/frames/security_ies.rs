//! Owned association RSN/RSNXE elements.
//!
//! Received bytes reach this module; nothing here panics on them.

use super::*;
use oer_protocols_ieee80211_wire::element::Element;

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
    bytes: [u8; N],
    length: oer_portable_wire::PrefixLength<N>,
}

impl<const N: usize> OwnedAssociationSecurityIes<N> {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn invalidate_for_test(&mut self) {
        self.length = oer_portable_wire::PrefixLength::ZERO;
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
        let length = rsn_ie
            .as_bytes()
            .len()
            .checked_add(rsnxe.len())
            .and_then(oer_portable_wire::PrefixLength::new)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let mut bytes = [0; N];
        // `length` proves both elements fit the array.
        for (target, source) in bytes.iter_mut().zip(rsn_ie.as_bytes().iter().chain(rsnxe)) {
            *target = *source;
        }
        Ok(Self { bytes, length })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }

    /// The array and the admitted length of its elements: their bound is
    /// the type's.
    pub(crate) const fn bounded(&self) -> (&[u8; N], oer_portable_wire::PrefixLength<N>) {
        (&self.bytes, self.length)
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
            let mut array = [0; 4];
            array[..bytes.len()].copy_from_slice(bytes);
            let owned = OwnedAssociationSecurityIes::<4> {
                bytes: array,
                length: oer_portable_wire::PrefixLength::new(bytes.len()).unwrap(),
            };
            assert_eq!(owned.rsn_ie(), Err(RsnFrameError::InvalidRsnIe));
            assert_eq!(owned.rsnxe(), Err(RsnFrameError::InvalidRsnIe));
        }
    }
}
