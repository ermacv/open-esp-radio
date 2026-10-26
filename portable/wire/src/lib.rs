#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

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
/// A field's type is an unsigned integer of at most 64 bits, `bool` (one bit),
/// or `[EnumName]`. Overlapping fields require an explicit `, view` suffix on
/// at least one field in each overlapping pair, for example
/// `pub combined, with_combined: u8 = 0..8, view;`. A view may appear before or
/// after the fields it aliases; it never permits two ordinary fields to overlap.
/// An enum declaration supplies `from_encoding` and `encoding`; unknown values
/// retain their complete encoding in `Reserved`. Optional conversion method names
/// can follow the raw type, as `(from_raw, raw)`. Numeric and enum setters return
/// `Result<Self, Error>`; a boolean setter is infallible. Invalid declarations
/// (range, type width, byte order, implicit overlap) fail at compile time.
#[macro_export]
macro_rules! bitfield {
    ($(#[$meta:meta])* $vis:vis enum $name:ident : $raw:ident {
        $($(#[$variant_meta:meta])* $variant:ident = $value:literal),* $(,)?
    }) => {
        $crate::bitfield! {
            $(#[$meta])* $vis enum $name: $raw (from_encoding, encoding) {
                $($(#[$variant_meta])* $variant = $value,)*
            }
        }
    };
    ($(#[$meta:meta])* $vis:vis enum $name:ident : $raw:ident ($decode:ident, $encode:ident) {
        $($(#[$variant_meta:meta])* $variant:ident = $value:literal),* $(,)?
    }) => {
        const _: () = { assert!($raw::MIN == 0 && $raw::BITS <= 64); };
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)*
            Reserved($raw),
        }
        impl $name {
            pub const BITS: usize = $raw::BITS as usize;
            pub const fn $decode(value: $raw) -> Self {
                match value { $($value => Self::$variant,)* raw => Self::Reserved(raw) }
            }
            pub const fn $encode(self) -> $raw {
                match self { $(Self::$variant => $value,)* Self::Reserved(raw) => raw }
            }
        }
    };
    ($(#[$meta:meta])* $vis:vis struct $name:ident [$size:literal] ($order:ident) {
        $($(#[$field_meta:meta])* $fvis:vis $get:ident, $set:ident : $ty:tt = $start:literal .. $end:literal $(, $view:ident)?;)*
    }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(transparent)]
        $vis struct $name([u8; $size]);
        const _: () = {
            assert!($size > 0);
            let _ = $crate::bitfield!(@order $order);
            $(
                assert!($start < $end && $end <= $size * 8);
                $crate::bitfield!(@width $ty, $end - $start);
            )*
            assert!($crate::__private::valid_overlaps(&[
                $(($start, $end, $crate::bitfield!(@view $($view)?)),)*
            ]), "overlapping wire fields require an explicit view");
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
    (@view) => { false };
    (@view view) => { true };
    (@width bool, $width:expr) => { assert!($width == 1); };
    (@width [$ty:ident], $width:expr) => { assert!($width <= $ty::BITS); };
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
            $crate::__private::get::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&self.0) != 0
        }
        $vis const fn $set(mut self, value: bool) -> Self {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start == 1); }
            $crate::__private::set::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&mut self.0, value as u64);
            self
        }
    };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: [$ty:ident],
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $ty {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= 64); }
            $ty::from_encoding($crate::__private::get::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&self.0) as _)
        }
        $vis const fn $set(mut self, value: $ty) -> Result<Self, $crate::Error> {
            let raw = value.encoding() as u64;
            if !$crate::__private::fits(raw, $end - $start) { return Err($crate::Error::ValueOutOfRange); }
            $crate::__private::set::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&mut self.0, raw);
            Ok(self)
        }
    };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: $ty:ident,
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $ty {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0); }
            $crate::__private::get::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&self.0) as $ty
        }
        $vis const fn $set(mut self, value: $ty) -> Result<Self, $crate::Error> {
            const { assert!($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0); }
            if !$crate::__private::fits(value as u64, $end - $start) { return Err($crate::Error::ValueOutOfRange); }
            $crate::__private::set::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&mut self.0, value as u64);
            Ok(self)
        }
    };
}

// Public only so exported macro expansions can reach it. All offsets come from
// compile-time-checked declarations, never from a peer.
#[doc(hidden)]
pub mod __private {
    pub const fn valid_overlaps(fields: &[(usize, usize, bool)]) -> bool {
        let mut remaining = fields;
        while let Some((&(start, end, view), rest)) = remaining.split_first() {
            let mut others = rest;
            while let Some((&(other_start, other_end, other_view), next)) = others.split_first() {
                if !view && !other_view && start < other_end && other_start < end {
                    return false;
                }
                others = next;
            }
            remaining = rest;
        }
        true
    }

    pub const fn fits(value: u64, width: usize) -> bool {
        width >= 64 || value < (1_u64 << width)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "const bounds prove START..END belongs to the fixed array"
    )]
    #[inline(always)]
    pub const fn get<const START: usize, const END: usize, const LITTLE: bool, const N: usize>(
        bytes: &[u8; N],
    ) -> u64 {
        const {
            assert!(START < END && END <= N * 8 && END - START <= 64);
        }
        let mut result = 0;
        let mut bit = START;
        while bit < END {
            let index = if LITTLE { bit / 8 } else { N - 1 - bit / 8 };
            let count = if END - bit < 8 - bit % 8 {
                END - bit
            } else {
                8 - bit % 8
            };
            let mask = (1_u16 << count) - 1;
            result |= (((bytes[index] as u16 >> (bit % 8)) & mask) as u64) << (bit - START);
            bit += count;
        }
        result
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "const bounds prove START..END belongs to the fixed array"
    )]
    #[inline(always)]
    pub const fn set<const START: usize, const END: usize, const LITTLE: bool, const N: usize>(
        bytes: &mut [u8; N],
        value: u64,
    ) {
        const {
            assert!(START < END && END <= N * 8 && END - START <= 64);
        }
        let mut bit = START;
        while bit < END {
            let index = if LITTLE { bit / 8 } else { N - 1 - bit / 8 };
            let count = if END - bit < 8 - bit % 8 {
                END - bit
            } else {
                8 - bit % 8
            };
            let mask = (((1_u16 << count) - 1) << (bit % 8)) as u8;
            bytes[index] =
                (bytes[index] & !mask) | (((value >> (bit - START)) as u8) << (bit % 8) & mask);
            bit += count;
        }
    }
}

#[cfg(test)]
mod tests;

/// Invalid wire declarations fail even when no accessor is called.
///
/// ```compile_fail
/// oer_wire::bitfield! { struct Beyond[1] (little) { value, with_value: u16 = 0..9; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { struct TooNarrow[2] (little) { value, with_value: u8 = 0..9; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { struct Boolean[1] (little) { value, with_value: bool = 0..2; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { struct Signed[1] (little) { value, with_value: i8 = 0..7; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { enum Kind: u8 { Data = 1 } }
/// oer_wire::bitfield! { struct EnumWidth[2] (little) { kind, with_kind: [Kind] = 0..9; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { struct Order[1] (native) { value, with_value: u8 = 0..8; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! { struct EmptyField[1] (little) { value, with_value: u8 = 1..1; } }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! {
///     struct Overlap[2] (little) {
///         first, with_first: u8 = 0..8;
///         second, with_second: u8 = 4..12;
///     }
/// }
/// ```
/// ```compile_fail
/// oer_wire::bitfield! {
///     struct ViewDoesNotPermitOtherOverlaps[1] (little) {
///         combined, with_combined: u8 = 0..8, view;
///         first, with_first: u8 = 0..4;
///         second, with_second: u8 = 3..7;
///     }
/// }
/// ```
#[cfg(doctest)]
pub struct InvalidDeclarations;
