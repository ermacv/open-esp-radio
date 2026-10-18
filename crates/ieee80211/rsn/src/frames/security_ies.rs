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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRsnIe<const N: usize = RSN_IE_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedRsnIe<N> {
    pub fn try_copy(bytes: &[u8]) -> Result<Self, RsnFrameError> {
        let Some((&[id, length], body)) = bytes.split_first_chunk::<2>() else {
            return Err(RsnFrameError::InvalidRsnIe);
        };
        if id != RSN_ELEMENT_ID || usize::from(length) != body.len() {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        let mut owned = [0; N];
        owned
            .get_mut(..bytes.len())
            .ok_or(RsnFrameError::CapacityExceeded)?
            .copy_from_slice(bytes);
        Ok(Self {
            len: bytes.len(),
            bytes: owned,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }
}

/// Exact RSN IE plus optional RSNXE emitted by the STA association request.
///
/// WPA2 message 2 must echo this complete byte sequence. Keeping it separate
/// from [`OwnedRsnIe`] preserves the stricter single-element invariant used
/// when validating message 3 key data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedAssociationSecurityIes<const N: usize = RSN_ASSOC_SECURITY_IES_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedAssociationSecurityIes<N> {
    /// Copy the exact RSN IE plus optional RSNXE emitted by Association.
    pub fn try_copy_bytes(bytes: &[u8]) -> Result<Self, RsnFrameError> {
        let Some(&[id, length]) = bytes.first_chunk::<2>() else {
            return Err(RsnFrameError::InvalidRsnIe);
        };
        if id != RSN_ELEMENT_ID {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        let Some((rsn, rsnxe)) = bytes.split_at_checked(usize::from(length) + 2) else {
            return Err(RsnFrameError::InvalidRsnIe);
        };
        let rsn = OwnedRsnIe::<RSN_IE_CAPACITY>::try_copy(rsn)?;
        Self::try_copy(&rsn, rsnxe)
    }

    pub fn try_copy<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        rsnxe: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if !rsnxe.is_empty() {
            let Some((&[id, length], body)) = rsnxe.split_first_chunk::<2>() else {
                return Err(RsnFrameError::InvalidRsnxe);
            };
            if id != RSNXE_ELEMENT_ID || usize::from(length) != body.len() {
                return Err(RsnFrameError::InvalidRsnxe);
            }
        }
        let len = rsn_ie
            .as_bytes()
            .len()
            .checked_add(rsnxe.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N {
            return Err(RsnFrameError::CapacityExceeded);
        }
        let mut bytes = [0; N];
        let (rsn, rest) = bytes
            .get_mut(..len)
            .ok_or(RsnFrameError::CapacityExceeded)?
            .split_at_mut(rsn_ie.as_bytes().len());
        rsn.copy_from_slice(rsn_ie.as_bytes());
        rest.copy_from_slice(rsnxe);
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }

    /// The RSN element and the RSNXE after it; both empty were the copy's
    /// validation not to hold.
    fn split(&self) -> (&[u8], &[u8]) {
        let bytes = self.as_bytes();
        bytes
            .get(1)
            .and_then(|&length| bytes.split_at_checked(usize::from(length) + 2))
            .unwrap_or((&[], &[]))
    }

    pub fn rsn_ie(&self) -> &[u8] {
        self.split().0
    }

    pub fn rsnxe(&self) -> &[u8] {
        self.split().1
    }
}
