#![no_std]
#![forbid(unsafe_code)]

//! Allocation-free wire fields and byte cursors shared by radio protocols.
//!
//! Bit ranges are half-open and count from the least significant bit of the
//! integer represented in the declared byte order. Setters reject values that
//! do not fit and retain every other bit, including reserved bits.

mod cursor;
pub use cursor::{Reader, Writer};

/// A refused cursor operation or field value. Failure never advances a cursor
/// or modifies its destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated { required: usize, remaining: usize },
    ValueOutOfRange,
}

/// Declare a fixed-width wire value, or a wire enum with `Reserved(raw)`.
///
/// ```
/// oer_wire::bitfield! {
///     /// SOURCE: IEEE Std 802.11-2020, 9.2.4.4 (Sequence Control field).
///     pub struct SequenceControl[2] (little) {
///         pub fragment, with_fragment: u8 = 0..4;
///         pub sequence, with_sequence: u16 = 4..16;
///     }
/// }
/// let value = SequenceControl::from_bytes([0; 2]).with_sequence(123)?;
/// assert_eq!(value.sequence(), 123);
/// assert!(value.with_fragment(16).is_err());
/// # Ok::<(), oer_wire::Error>(())
/// ```
///
/// A field's type is an unsigned integer, `bool` (one bit), or `[EnumName]`.
/// An enum declaration supplies `from_encoding` and `encoding`; unknown values
/// retain their complete encoding in `Reserved`. Numeric and enum setters return
/// `Result<Self, Error>`; a boolean setter is infallible. Invalid declarations
/// (range, type width, byte order) fail at compile time.
#[macro_export]
macro_rules! bitfield {
    ($(#[$meta:meta])* $vis:vis enum $name:ident : $raw:ident {
        $($(#[$variant_meta:meta])* $variant:ident = $value:literal),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)*
            Reserved($raw),
        }
        impl $name {
            pub const fn from_encoding(value: $raw) -> Self {
                match value { $($value => Self::$variant,)* raw => Self::Reserved(raw) }
            }
            pub const fn encoding(self) -> $raw {
                match self { $(Self::$variant => $value,)* Self::Reserved(raw) => raw }
            }
        }
    };
    ($(#[$meta:meta])* $vis:vis struct $name:ident [$size:literal] ($order:ident) {
        $($(#[$field_meta:meta])* $fvis:vis $get:ident, $set:ident : $ty:tt = $start:literal .. $end:literal;)*
    }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $vis struct $name([u8; $size]);
        const _: () = {
            assert!($size > 0);
            $(
                assert!($start < $end && $end <= $size * 8);
                $crate::bitfield!(@width $ty, $end - $start);
            )*
        };
        impl $name {
            pub const fn from_bytes(bytes: [u8; $size]) -> Self { Self(bytes) }
            pub const fn to_bytes(self) -> [u8; $size] { self.0 }
            $(
                $crate::bitfield!(@field $(#[$field_meta])* $fvis $get, $set: $ty,
                    $start, $end, $size, $order);
            )*
        }
    };
    (@width bool, $width:expr) => { assert!($width == 1); };
    (@width [$ty:ident], $width:expr) => { assert!($width <= 64); };
    (@width $ty:ident, $width:expr) => {
        assert!($ty::MIN == 0 && $ty::BITS <= 64 && $width <= $ty::BITS as usize);
    };
    (@order little) => { true };
    (@order big) => { false };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: bool,
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> bool {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start == 1); }
            $crate::__private::get(&self.0, $start, $end, $crate::bitfield!(@order $order)) != 0
        }
        $vis const fn $set(mut self, value: bool) -> Self {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start == 1); }
            $crate::__private::set(&mut self.0, $start, $end, value as u64, $crate::bitfield!(@order $order));
            self
        }
    };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: [$ty:ident],
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $ty {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= 64); }
            $ty::from_encoding($crate::__private::get(&self.0, $start, $end, $crate::bitfield!(@order $order)) as _)
        }
        $vis const fn $set(mut self, value: $ty) -> Result<Self, $crate::Error> {
            let raw = value.encoding() as u64;
            if !$crate::__private::fits(raw, $end - $start) { return Err($crate::Error::ValueOutOfRange); }
            $crate::__private::set(&mut self.0, $start, $end, raw, $crate::bitfield!(@order $order));
            Ok(self)
        }
    };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: $ty:ident,
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $ty {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0); }
            $crate::__private::get(&self.0, $start, $end, $crate::bitfield!(@order $order)) as $ty
        }
        $vis const fn $set(mut self, value: $ty) -> Result<Self, $crate::Error> {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0); }
            if !$crate::__private::fits(value as u64, $end - $start) { return Err($crate::Error::ValueOutOfRange); }
            $crate::__private::set(&mut self.0, $start, $end, value as u64, $crate::bitfield!(@order $order));
            Ok(self)
        }
    };
}

// Public only so exported macro expansions can reach it. All offsets come from
// compile-time-checked declarations, never from a peer.
#[doc(hidden)]
pub mod __private {
    pub const fn fits(value: u64, width: usize) -> bool {
        width == 64 || value < (1_u64 << width)
    }

    #[inline(always)]
    pub const fn get(bytes: &[u8], start: usize, end: usize, little: bool) -> u64 {
        let mut result = 0;
        let mut bit = start;
        while bit < end {
            let index = if little {
                bit / 8
            } else {
                bytes.len() - 1 - bit / 8
            };
            result |= (((bytes[index] >> (bit % 8)) & 1) as u64) << (bit - start);
            bit += 1;
        }
        result
    }

    #[inline(always)]
    pub const fn set(bytes: &mut [u8], start: usize, end: usize, value: u64, little: bool) {
        let mut bit = start;
        while bit < end {
            let index = if little {
                bit / 8
            } else {
                bytes.len() - 1 - bit / 8
            };
            let mask = 1_u8 << (bit % 8);
            bytes[index] =
                (bytes[index] & !mask) | ((((value >> (bit - start)) & 1) as u8) << (bit % 8));
            bit += 1;
        }
    }
}

#[cfg(test)]
mod tests;
