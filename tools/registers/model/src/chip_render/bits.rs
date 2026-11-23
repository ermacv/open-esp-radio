//! The field value type of the generated root: `Bits` and its error,
//! emitted once into the PAC, whose generated getters alone cut a value.

/// The text of `Bits`, `ValueError` and their lossless conversions.
pub(super) const BITS: &str = r##"/// An unsigned value of exactly `WIDTH` bits: a field without a narrower
/// domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Bits<const WIDTH: u32>(u32);

/// A value wider than its field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueError {
    /// The refused value.
    pub value: u32,
    /// The field's width in bits.
    pub width: u32,
}

impl<const WIDTH: u32> Bits<WIDTH> {
    /// The largest value.
    pub const MAX: u32 = const {
        if !(WIDTH >= 1 && WIDTH <= 32) {
            panic!("a field is 1 to 32 bits wide");
        };
        u32::MAX.wrapping_shr(32_u32.wrapping_sub(WIDTH))
    };

    /// The value `value`, if it fits `WIDTH` bits.
    pub const fn new(value: u32) -> Result<Self, ValueError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(ValueError {
                value,
                width: WIDTH,
            })
        }
    }

    /// The value `V`, checked at compile time.
    pub const fn constant<const V: u32>() -> Self {
        const {
            if !(V <= Self::MAX) {
                panic!("the value fits the field");
            }
        };
        Self(V)
    }

    /// The low `WIDTH` bits of `value`: the field cut out of a word a
    /// getter read, or a window's or a vendor narrowing's bits. It keeps the
    /// low bits of any value, so it is the PAC's own: outside it a field's
    /// value is a name, a constant, the checked `new` or a conversion that
    /// loses nothing.
    pub(crate) const fn cut(value: u32) -> Self {
        Self(value & Self::MAX)
    }

    /// The value.
    pub const fn get(self) -> u32 {
        self.0
    }

    /// `SCALE · index + BIAS`: a constant times a bounded number plus a
    /// constant, whose largest value the field holds, checked where it is
    /// monomorphized. A field too narrow refuses to build.
    pub const fn affine<const SCALE: u32, const BIAS: u32, const COUNT: usize>(
        index: oer_portable_bounded::Index<COUNT>,
    ) -> Self {
        const {
            if COUNT == 0 || COUNT as u64 > 1_u64 << 32 {
                panic!("an index type has a number and its numbers are 32-bit");
            }
            let largest = (COUNT as u64 - 1) * SCALE as u64 + BIAS as u64;
            if largest > Self::MAX as u64 {
                panic!("the field holds the largest value");
            }
        };
        // The sum fits by the check above.
        Self(index.to_u32().wrapping_mul(SCALE).wrapping_add(BIAS))
    }

    /// The value as its low `LOW` bits and its high `HIGH` bits, the two
    /// fields one number is written to: `LOW + HIGH` is the width, checked
    /// where it is monomorphized, so no bit is lost.
    pub const fn split<const LOW: u32, const HIGH: u32>(self) -> (Bits<LOW>, Bits<HIGH>) {
        const {
            if LOW == 0 || HIGH == 0 || LOW as u64 + HIGH as u64 != WIDTH as u64 {
                panic!("the two parts are the whole value");
            }
        };
        (
            Bits(self.0 & Bits::<LOW>::MAX),
            Bits(self.0.wrapping_shr(LOW)),
        )
    }

    /// The little-endian bytes `bytes` as one value: `8 · N` bits, which
    /// the field holds, checked where it is monomorphized.
    pub fn from_le_bytes<const N: usize>(bytes: [u8; N]) -> Self {
        const {
            if N == 0 || N > 4 || (N as u64) * 8 > WIDTH as u64 {
                panic!("the field holds every byte");
            }
        };
        let mut word = [0_u8; 4];
        for (to, from) in word.iter_mut().zip(bytes) {
            *to = from;
        }
        Self(u32::from_le_bytes(word))
    }
}

/// A byte into a field of at least eight bits: every value fits, so the
/// conversion loses nothing. A narrower field refuses to build.
impl<const WIDTH: u32> From<u8> for Bits<WIDTH> {
    fn from(value: u8) -> Self {
        const {
            if !(WIDTH >= 8 && WIDTH <= 32) {
                panic!("a byte fits a field of 8 to 32 bits");
            }
        };
        Self(u32::from(value))
    }
}

/// A halfword into a field of at least sixteen bits: every value fits. A
/// narrower field refuses to build.
impl<const WIDTH: u32> From<u16> for Bits<WIDTH> {
    fn from(value: u16) -> Self {
        const {
            if !(WIDTH >= 16 && WIDTH <= 32) {
                panic!("a halfword fits a field of 16 to 32 bits");
            }
        };
        Self(u32::from(value))
    }
}

/// An index below `COUNT` into a field that holds every index: `COUNT` is
/// at most `2^WIDTH`. A field too narrow for the array refuses to build.
impl<const COUNT: usize, const WIDTH: u32> From<oer_portable_bounded::Index<COUNT>> for Bits<WIDTH> {
    fn from(index: oer_portable_bounded::Index<COUNT>) -> Self {
        const {
            if !(WIDTH >= 1 && WIDTH <= 32) {
                panic!("a field is 1 to 32 bits wide");
            }
            if !(COUNT as u64 <= 1_u64 << WIDTH) {
                panic!("the field holds every index of the array");
            }
        };
        Self(index.to_u32())
    }
}

impl<const WIDTH: u32> From<Bits<WIDTH>> for u32 {
    fn from(bits: Bits<WIDTH>) -> Self {
        bits.0
    }
}
"##;
