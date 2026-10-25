//! The analog-I2C transport space: a byte's address is its block's code and
//! its index, never a CPU address; a route profile states how a request
//! reaches the byte.

use serde::{Deserialize, Serialize};

/// One analog-I2C byte: its block's code and its index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalogAddress {
    pub block: u8,
    pub index: u8,
}

impl AnalogAddress {
    /// Canonical numeric transport identity for the semantic-ID grammar.
    /// This encoding does not create a CPU address or pointer.
    pub const fn identity(self) -> u64 {
        ((self.block as u64) << 8) | self.index as u64
    }
}

/// How a request with `request_code` reaches a block in one transport
/// profile.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Route {
    pub profile: String,
    pub request_code: u8,
    pub host: u8,
    pub read_select: u16,
    pub read_mask_bits: u8,
    pub sources: Vec<String>,
}
