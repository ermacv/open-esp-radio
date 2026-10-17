//! Validated binary IEEE 802.11 service-set identifiers.

use core::fmt;

use crate::management::MAX_SSID_LEN;

/// Binary IEEE 802.11 SSID with its protocol length validated once.
///
/// An SSID is not required to be UTF-8. The same value is shared by station,
/// access-point and scan policy instead of making those roles depend on each
/// other solely for a wire-protocol type.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct WifiSsid {
    bytes: [u8; MAX_SSID_LEN],
    length: u8,
}

impl WifiSsid {
    pub fn new(bytes: &[u8]) -> Result<Self, WifiSsidError> {
        if bytes.is_empty() {
            return Err(WifiSsidError::Empty);
        }
        if bytes.len() > MAX_SSID_LEN {
            return Err(WifiSsidError::TooLong {
                length: bytes.len(),
                maximum: MAX_SSID_LEN,
            });
        }
        let mut stored = [0; MAX_SSID_LEN];
        stored[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: stored,
            length: bytes.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    pub const fn len(&self) -> usize {
        self.length as usize
    }

    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Debug for WifiSsid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiSsid")
            .field("bytes", &self.as_bytes())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSsidError {
    Empty,
    TooLong { length: usize, maximum: usize },
}

impl fmt::Display for WifiSsidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("an IEEE 802.11 SSID cannot be empty"),
            Self::TooLong { length, maximum } => write!(
                formatter,
                "IEEE 802.11 SSID contains {length} bytes; the maximum is {maximum}"
            ),
        }
    }
}

impl core::error::Error for WifiSsidError {}

#[cfg(test)]
mod tests;
