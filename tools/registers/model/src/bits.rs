//! The same declaration states for direct and indirect registers.
use crate::{Error, Result};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BitStates {
    pub described: u128,
    pub opaque: u128,
    pub reserved: u128,
    pub absent: u128,
}

impl BitStates {
    /// Unknown analysis/reset is evidence status, never another declaration
    /// state. Unnamed gaps stay absent, rather than becoming reserved.
    pub fn new(width: u32, fields: u128, opaque: u128, reserved: u128) -> Result<Self> {
        if !(1..=128).contains(&width) {
            return Err(Error::message("invalid register width"));
        }
        let all = if width == 128 {
            u128::MAX
        } else {
            (1u128 << width) - 1
        };
        if (fields | opaque | reserved) & !all != 0
            || opaque & !fields != 0
            || fields & reserved != 0
        {
            return Err(Error::message("bit states exceed width or overlap"));
        }
        Ok(Self {
            described: fields & !opaque,
            opaque,
            reserved,
            absent: all & !(fields | reserved),
        })
    }
}
