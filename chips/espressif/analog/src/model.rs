//! Shared vocabulary for generated analog-I2C models. These values describe
//! transport identities and sampled bytes; they perform no hardware access.
use crate::AnalogField;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KnownReset {
    value: u8,
    mask: u8,
}

impl KnownReset {
    pub const fn new(value: u8, mask: u8) -> Option<Self> {
        if mask != 0 && value & !mask == 0 {
            Some(Self { value, mask })
        } else {
            None
        }
    }
    pub const fn value(self) -> u8 {
        self.value
    }
    pub const fn mask(self) -> u8 {
        self.mask
    }
    /// A partial or unknown reset never supplies a complete write image.
    pub const fn whole_image(self) -> Option<u8> {
        if self.mask == 0xff {
            Some(self.value)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterFacts {
    pub address: ByteAddress,
    pub described: u8,
    pub opaque: u8,
    pub reserved: u8,
    pub reset: Option<KnownReset>,
    pub read_safe_epoch: Option<&'static str>,
}

/// One byte admitted by a finite diagnostic epoch, with independently
/// reviewed comparison lanes. The mask supplies no reset or bit semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureRegister {
    pub address: ByteAddress,
    pub mask: u8,
    pub epoch: &'static str,
}

impl RegisterFacts {
    pub const fn absent(self) -> u8 {
        !(self.described | self.opaque | self.reserved)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SampledByte {
    address: ByteAddress,
    bits: u8,
}

impl SampledByte {
    pub const fn new(address: ByteAddress, bits: u8) -> Self {
        Self { address, bits }
    }
    pub const fn bits(self) -> u8 {
        self.bits
    }
    pub const fn address(self) -> ByteAddress {
        self.address
    }
    const fn matches(self, field: AnalogField<ByteAddress>) -> bool {
        let address = field.address();
        self.address.block == address.block && self.address.index == address.index
    }
    pub const fn field(self, field: AnalogField<ByteAddress>) -> Option<u8> {
        if self.matches(field) {
            Some(field.extract(self.bits))
        } else {
            None
        }
    }
    pub const fn with_field(self, field: AnalogField<ByteAddress>, value: u8) -> Option<Self> {
        if !self.matches(field) {
            return None;
        }
        match field.insert(self.bits, value) {
            Some(bits) => Some(Self::new(self.address, bits)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_truncation_preserves_the_existing_byte_input_contract() {
        for lsb in 0..8 {
            for msb in lsb..8 {
                let field = AnalogField::new((), msb, lsb).unwrap();
                let mask = (((1_u16 << (msb - lsb + 1)) - 1) << lsb) as u8;
                for sample in 0..=255_u8 {
                    for value in 0..=255_u8 {
                        let result = field.insert_truncated(sample, value);
                        assert_eq!(result & !mask, sample & !mask);
                        assert_eq!(u16::from(field.extract(result)), u16::from(value) % (1 << (msb - lsb + 1)));
                        if let Some(checked) = field.insert(sample, value) {
                            assert_eq!(checked, result);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn sampled_values_preserve_other_lanes_and_reject_another_register_or_overflow() {
        let address = ByteAddress::new(0x66, 1);
        let field = AnalogField::new(address, 3, 1).unwrap();
        let sample = SampledByte::new(address, 0xa5);
        assert_eq!(sample.with_field(field, 7).unwrap().bits(), 0xaf);
        assert_eq!(sample.with_field(field, 8), None);
        assert_eq!(
            SampledByte::new(ByteAddress::new(0x66, 2), 0).with_field(field, 1),
            None
        );
        assert_eq!(KnownReset::new(3, 0x0f).unwrap().whole_image(), None);
        assert_eq!(KnownReset::new(0x83, 0x0f), None);
        assert_eq!(KnownReset::new(3, 0xff).unwrap().whole_image(), Some(3));
    }
}
