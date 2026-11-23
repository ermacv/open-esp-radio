#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
//! The vocabulary of the Espressif analog register space: a field of one
//! byte register, the block codes, the byte addresses, the two hosts of the
//! analog I2C master and the routes a chip's model gives each block.
//!
//! These are values about the hardware; they perform no access. A chip's
//! PAC writes its generated `analog_i2c` dictionary in them, and the bus
//! driver (`oer-chips-family-drivers-regi2c`) takes their fields.

#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

/// Bits `msb..=lsb` of one analog byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalogField<Address> {
    address: Address,
    msb: u8,
    lsb: u8,
    mask: u8,
}

impl<Address: Copy> AnalogField<Address> {
    /// The field, when `lsb <= msb <= 7`.
    pub const fn new(address: Address, msb: u8, lsb: u8) -> Option<Self> {
        if lsb <= msb && msb < 8 {
            let Some(upper_shift) = 7_u8.checked_sub(msb) else {
                return None;
            };
            let mask = (u8::MAX << lsb) & (u8::MAX >> upper_shift);
            Some(Self {
                address,
                msb,
                lsb,
                mask,
            })
        } else {
            None
        }
    }

    pub const fn bit_range(self) -> (u8, u8) {
        (self.msb, self.lsb)
    }

    pub const fn address(self) -> Address {
        self.address
    }

    /// Whether `other` lies in the same bits of its byte: a value of one
    /// applies to the other.
    pub const fn same_lanes<Other: Copy>(self, other: AnalogField<Other>) -> bool {
        self.msb == other.msb && self.lsb == other.lsb
    }

    /// The field's value in `byte`, as `phy_i2c_readReg_Mask` extracts it.
    pub const fn extract(self, byte: u8) -> u8 {
        (byte & self.mask) >> self.lsb
    }

    /// `value` in the field's place, when it fits the field; `None`
    /// otherwise. A constant is built in a const item, which then refuses
    /// to build when the value does not fit.
    ///
    /// The vendor leaf (`phy_i2c_writeReg_Mask`) ORs `value << lsb` in
    /// unmasked, so a wider value would also set bits above the field; such
    /// values are refused here.
    pub const fn place(self, value: u8) -> Option<FieldValue<Address>> {
        if value & !(self.mask >> self.lsb) != 0 {
            return None;
        }
        Some(FieldValue {
            field: self,
            bits: value << self.lsb,
        })
    }

    /// The field's value as `byte` holds it: it fits by construction.
    pub const fn value_in(self, byte: u8) -> FieldValue<Address> {
        FieldValue {
            field: self,
            bits: byte & self.mask,
        }
    }

    /// Replace the field with the low bits of `value`, preserving every
    /// other lane. Callers that explicitly truncate a byte input use this;
    /// checked transactions use [`Self::insert`].
    pub const fn insert_truncated(self, byte: u8, value: u8) -> u8 {
        (byte & !self.mask) | ((value << self.lsb) & self.mask)
    }
}

/// A field's value in its place: the field it was placed for (its byte
/// register's address and its lanes) and the bits it holds there. Only a
/// field makes one ([`AnalogField::place`], [`AnalogField::value_in`]), so
/// it always fits its lanes and names the register it is written to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldValue<Address> {
    field: AnalogField<Address>,
    bits: u8,
}

impl<Address: Copy> FieldValue<Address> {
    /// The field the value was placed for.
    pub const fn field(self) -> AnalogField<Address> {
        self.field
    }

    /// The byte register the value is written to: its field's.
    pub const fn address(self) -> Address {
        self.field.address
    }

    /// `byte` with the field's lanes replaced by this value and every other
    /// lane kept, as `phy_i2c_writeReg_Mask` composes it.
    pub const fn apply(self, byte: u8) -> u8 {
        (byte & !self.field.mask) | self.bits
    }

    /// The value, unshifted.
    pub const fn value(self) -> u8 {
        self.bits >> self.field.lsb
    }
}

/// One analog block's code.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BlockCode(u8);

impl BlockCode {
    pub const fn new(code: u8) -> Self {
        Self(code)
    }
    pub const fn code(self) -> u8 {
        self.0
    }
    pub const fn register(self, index: u8) -> ByteAddress {
        ByteAddress::new(self.0, index)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ByteAddress {
    block: u8,
    index: u8,
}

impl ByteAddress {
    pub const fn new(block: u8, index: u8) -> Self {
        Self { block, index }
    }
    pub const fn block(self) -> u8 {
        self.block
    }
    pub const fn index(self) -> u8 {
        self.index
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Host {
    Host0,
    Host1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Route {
    pub request_code: u8,
    pub command_code: u8,
    pub host: Host,
    /// Slave selection, not a mask of readable data bits.
    pub read_select: u16,
    pub read_mask_bits: u8,
}

impl Route {
    pub const fn address(self, index: u8) -> ByteAddress {
        ByteAddress::new(self.command_code, index)
    }
    pub const fn complemented_read_select(self) -> u32 {
        let mask = if self.read_mask_bits == 32 {
            u32::MAX
        } else {
            0x00ff_ffff
        };
        mask & !(self.read_select as u32)
    }
}

/// One byte admitted by a finite diagnostic epoch, with independently
/// reviewed comparison lanes. The mask supplies no reset or bit semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureRegister {
    pub address: ByteAddress,
    pub mask: u8,
    pub epoch: &'static str,
}

#[cfg(test)]
mod tests;
