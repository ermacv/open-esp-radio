//! Owned association RSN/RSNXE elements.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRsnIe<const N: usize = RSN_IE_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedRsnIe<N> {
    pub fn try_copy(bytes: &[u8]) -> Result<Self, RsnFrameError> {
        if bytes.len() < 2 || bytes[0] != RSN_ELEMENT_ID || bytes[1] as usize + 2 != bytes.len() {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        if bytes.len() > N {
            return Err(RsnFrameError::CapacityExceeded);
        }
        let mut owned = [0; N];
        owned[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            len: bytes.len(),
            bytes: owned,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
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
        if bytes.len() < 2 || bytes[0] != RSN_ELEMENT_ID {
            return Err(RsnFrameError::InvalidRsnIe);
        }
        let rsn_len = bytes[1] as usize + 2;
        let Some(rsn) = bytes.get(..rsn_len) else {
            return Err(RsnFrameError::InvalidRsnIe);
        };
        let rsn = OwnedRsnIe::<RSN_IE_CAPACITY>::try_copy(rsn)?;
        let rsnxe = bytes.get(rsn_len..).ok_or(RsnFrameError::InvalidRsnxe)?;
        Self::try_copy(&rsn, rsnxe)
    }

    pub fn try_copy<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        rsnxe: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if !rsnxe.is_empty()
            && (rsnxe.len() < 2
                || rsnxe[0] != RSNXE_ELEMENT_ID
                || rsnxe[1] as usize + 2 != rsnxe.len())
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
        let mut bytes = [0; N];
        let rsn_len = rsn_ie.as_bytes().len();
        bytes[..rsn_len].copy_from_slice(rsn_ie.as_bytes());
        bytes[rsn_len..len].copy_from_slice(rsnxe);
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn rsn_ie(&self) -> &[u8] {
        let length = self.bytes[1] as usize + 2;
        &self.bytes[..length]
    }

    pub fn rsnxe(&self) -> &[u8] {
        let length = self.bytes[1] as usize + 2;
        &self.bytes[length..self.len]
    }
}
