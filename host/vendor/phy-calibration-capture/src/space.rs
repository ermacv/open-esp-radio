//! The register spaces of the cross-check's images: memory-mapped words
//! and analog-I2C bytes, each read through the vendor firmware's console
//! protocol ([`crate::vendor`]).
use crate::Result;

/// The register space a [`Register`] address lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// A memory-mapped radio-PHY register word.
    Mmio,
    /// An analog-I2C byte register, addressed by [`analog_address`].
    Analog,
}

impl Space {
    /// The vendor firmware's request for the register at `address`.
    pub fn request(self, address: u32) -> String {
        match self {
            Self::Mmio => crate::vendor::register_request(address),
            Self::Analog => {
                let (block, register) = analog_parts(address);
                crate::vendor::analog_request(block, register)
            }
        }
    }

    /// The reply line reporting `value` at `address`.
    pub fn line(self, address: u32, value: u32) -> String {
        match self {
            Self::Mmio => crate::vendor::register_line(address, value),
            Self::Analog => {
                let (block, register) = analog_parts(address);
                crate::vendor::analog_line(block, register, value as u8)
            }
        }
    }

    /// Every complete reply of `console` in this space, by address.
    pub fn replies(self, console: &str) -> Result<std::collections::BTreeMap<u32, u32>> {
        Ok(match self {
            Self::Mmio => crate::vendor::registers(console)?,
            Self::Analog => crate::vendor::analog(console)?
                .into_iter()
                .map(|((block, register), value)| {
                    (analog_address(block, register), u32::from(value))
                })
                .collect(),
        })
    }
}

/// The [`Register`] address of analog `register` of `block`.
pub fn analog_address(block: u8, register: u8) -> u32 {
    u32::from_be_bytes([0, 0, block, register])
}

/// The block and register of an analog [`Register`] address.
pub fn analog_parts(address: u32) -> (u8, u8) {
    let [_, _, block, register] = address.to_be_bytes();
    (block, register)
}

/// One register of the image, named by its published peripheral and
/// register (and array element).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub name: String,
    pub address: u32,
}
