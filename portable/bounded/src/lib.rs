//! Bounded numbers: a number below a bound, made only by a checked
//! constructor or a conversion that loses nothing. The register code takes
//! its array indices from here, and the sans-IO layers their bounded
//! values (a priority of sixteen, the last word of a chunk), so a bound
//! proven once crosses every layer as a type.

#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods, clippy::disallowed_macros))]

/// A number below `COUNT`: an element of a register array, or any number
/// the type bounds (a lane, the last word of a chunk, a table entry).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Index<const COUNT: usize>(usize);

/// An index outside its array.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexError {
    /// The refused index.
    pub index: usize,
    /// The array's element count.
    pub count: usize,
}

impl<const COUNT: usize> Index<COUNT> {
    /// The index `index`, if it is below `COUNT`.
    pub const fn new(index: usize) -> Result<Self, IndexError> {
        if index < COUNT {
            Ok(Self(index))
        } else {
            Err(IndexError {
                index,
                count: COUNT,
            })
        }
    }

    /// The index `I`, checked at compile time: the bound itself refuses
    /// to build (`tests/ui/constant_at_the_bound.rs`).
    pub const fn constant<const I: usize>() -> Self {
        const {
            if !(I < COUNT) {
                panic!("the index lies inside the array");
            }
        };
        Self(I)
    }

    /// The index as a number below `COUNT`.
    pub const fn get(self) -> usize {
        self.0
    }

    /// The number as a byte: every number below `COUNT` is one, checked
    /// where it is monomorphized (`tests/ui/a_byte_of_a_wider_bound.rs`).
    pub const fn to_u8(self) -> u8 {
        const {
            if COUNT > 1 << 8 {
                panic!("every number below the bound is a byte");
            }
        };
        // Below 256 (checked above): the low byte is the whole number.
        let [byte, ..] = self.0.to_le_bytes();
        byte
    }

    /// The number as a 32-bit word: every number below `COUNT` is one,
    /// checked where it is monomorphized.
    pub const fn to_u32(self) -> u32 {
        const {
            if COUNT as u64 > 1 << 32 {
                panic!("every number below the bound is a 32-bit word");
            }
        };
        // Below 2^32 (checked above): the low four bytes are the whole
        // number, on a 32- or a 64-bit target.
        let [b0, b1, b2, b3, ..] = self.0.to_le_bytes();
        u32::from_le_bytes([b0, b1, b2, b3])
    }

    /// The byte `value`, when every byte is below `COUNT`. A smaller bound
    /// refuses to build:
    ///
    /// ```compile_fail,E0080
    /// let index = oer_portable_bounded::Index::<255>::from_byte(7);
    /// # let _ = index;
    /// ```
    pub const fn from_byte(value: u8) -> Self {
        const {
            if COUNT < 256 {
                panic!("every byte is below the bound");
            }
        };
        Self(value as usize)
    }
}

#[cfg(test)]
mod tests;
