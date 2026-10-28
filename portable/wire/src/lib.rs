#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Allocation-free wire fields and byte cursors shared by radio protocols.
//!
//! Bit ranges are half-open and count from the least significant bit of the
//! integer represented in the declared byte order. Setters reject values that
//! do not fit and retain every other bit, including reserved bits.

mod value;
pub use value::{PrefixLength, U8, U16, U32, U64};

mod cursor;
pub use cursor::{Reader, Writer};

/// A refused cursor operation or field value. Failure never advances a cursor
/// or modifies its destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated { required: usize, remaining: usize },
    ValueOutOfRange,
    LengthOverflow,
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
/// `Result<Self, Error>`; a boolean setter is infallible. Parenthesized integer
/// types, such as `(u16)`, return a width-proved [`U16`] from the getter and
/// take that same type in an infallible setter. Invalid declarations
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
        const _: () = {
            if !($raw::MIN == 0 && $raw::BITS <= 64) {
                panic!("wire enum requires an unsigned integer of at most 64 bits");
            }
        };
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
            if !($size > 0) { panic!("wire value must contain at least one byte"); }
            let _ = $crate::bitfield!(@order $order);
            $(
                if !($start < $end && $end <= $size * 8) { panic!("wire field must be nonempty and fit its array"); }
                $crate::bitfield!(@width $ty, $end - $start);
            )*
            if !$crate::__private::valid_overlaps(&[
                $(($start, $end, $crate::bitfield!(@view $($view)?)),)*
            ]) {
                panic!("overlapping wire fields require an explicit view");
            }
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
    (@width bool, $width:expr) => {
        const { if !($width == 1) { panic!("boolean wire field must contain one bit"); } }
    };
    (@width [$ty:ident], $width:expr) => {
        const { if !($width <= $ty::BITS) { panic!("wire field exceeds its enum width"); } }
    };
    (@width $ty:ident, $width:expr) => {
        const {
            if !($ty::MIN == 0 && $ty::BITS <= 64 && $width <= $ty::BITS as usize) {
                panic!("wire field requires a fitting unsigned integer of at most 64 bits");
            }
        }
    };
    (@order little) => { true };
    (@order big) => { false };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: bool,
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> bool {
            const { if !($start < $end && $end <= $size * 8 && $end - $start == 1) { panic!("invalid wire field geometry or value"); } }
            $crate::__private::get::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&self.0) != 0
        }
        $vis const fn $set(mut self, value: bool) -> Self {
            const { if !($start < $end && $end <= $size * 8 && $end - $start == 1) { panic!("invalid wire field geometry or value"); } }
            $crate::__private::set::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&mut self.0, value as u64);
            self
        }
    };
    // Parenthesized integers retain the width proof through reads and writes.
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: ($ty:ident),
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $crate::bitfield!(@bounded $ty, {$end - $start}) {
            <$crate::bitfield!(@bounded $ty, {$end - $start})>::from_field::<
                $start, $end, {$crate::bitfield!(@order $order)}, $size
            >(&self.0)
        }
        $vis const fn $set(mut self, value: $crate::bitfield!(@bounded $ty, {$end - $start})) -> Self {
            $crate::__private::set::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&mut self.0, value.get() as u64);
            self
        }
    };
    (@bounded u8, $width:expr) => { $crate::U8<$width> };
    (@bounded u16, $width:expr) => { $crate::U16<$width> };
    (@bounded u32, $width:expr) => { $crate::U32<$width> };
    (@bounded u64, $width:expr) => { $crate::U64<$width> };
    (@width ($ty:ident), $width:expr) => {
        const { if !($width <= $ty::BITS as usize) { panic!("wire field exceeds its integer width"); } }
    };
    (@field $(#[$meta:meta])* $vis:vis $get:ident, $set:ident: [$ty:ident],
        $start:literal, $end:literal, $size:literal, $order:ident) => {
        $(#[$meta])*
        $vis const fn $get(self) -> $ty {
            const { if !($start < $end && $end <= $size * 8 && $end - $start <= 64) { panic!("invalid wire field geometry or value"); } }
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
            const { if !($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0) { panic!("invalid wire field geometry or value"); } }
            $crate::__private::get::<$start, $end, {$crate::bitfield!(@order $order)}, $size>(&self.0) as $ty
        }
        $vis const fn $set(mut self, value: $ty) -> Result<Self, $crate::Error> {
            const { if !($start < $end && $end <= $size * 8 && $end - $start <= $ty::BITS as usize && $ty::MIN == 0) { panic!("invalid wire field geometry or value"); } }
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

    // The array and const geometry prove that every field octet is present.
    // Traversal skips lower octets and stops at the field's end, retaining
    // surrounding bits without any panicking slice operations. Saturating
    // counter steps cannot saturate: each step consumes admitted octets/bits.
    #[inline(always)]
    pub const fn get<const START: usize, const END: usize, const LITTLE: bool, const N: usize>(
        bytes: &[u8; N],
    ) -> u64 {
        const {
            if !(START < END && END <= N * 8 && END - START <= 64) {
                panic!("invalid wire field geometry or value");
            }
        }
        let mut bytes = bytes.as_slice();
        let mut skip = const { START / 8 };
        let mut result = 0;
        let mut shift = const { START % 8 };
        let mut remaining = const { END - START };
        let mut destination = 0_usize;
        while let Some((&byte, rest)) = if LITTLE {
            bytes.split_first()
        } else {
            bytes.split_last()
        } {
            bytes = rest;
            if skip != 0 {
                skip = skip.saturating_sub(1);
                continue;
            }
            let available = 8_usize.saturating_sub(shift);
            let count = if remaining < available {
                remaining
            } else {
                available
            };
            let mask = u8::MAX >> 8_usize.saturating_sub(count);
            result |= (((byte >> shift) & mask) as u64) << destination;
            remaining = remaining.saturating_sub(count);
            if remaining == 0 {
                break;
            }
            destination = destination.saturating_add(count);
            shift = 0;
        }
        result
    }

    #[inline(always)]
    pub const fn set<const START: usize, const END: usize, const LITTLE: bool, const N: usize>(
        bytes: &mut [u8; N],
        value: u64,
    ) {
        const {
            if !(START < END && END <= N * 8 && END - START <= 64) {
                panic!("invalid wire field geometry or value");
            }
        }
        let mut bytes = bytes.as_mut_slice();
        let mut skip = const { START / 8 };
        let mut shift = const { START % 8 };
        let mut remaining = const { END - START };
        let mut source = 0_usize;
        while let Some((byte, rest)) = if LITTLE {
            bytes.split_first_mut()
        } else {
            bytes.split_last_mut()
        } {
            bytes = rest;
            if skip != 0 {
                skip = skip.saturating_sub(1);
                continue;
            }
            let available = 8_usize.saturating_sub(shift);
            let count = if remaining < available {
                remaining
            } else {
                available
            };
            let mask = (u8::MAX >> 8_usize.saturating_sub(count)) << shift;
            *byte = (*byte & !mask) | (((value >> source) as u8) << shift & mask);
            remaining = remaining.saturating_sub(count);
            if remaining == 0 {
                break;
            }
            source = source.saturating_add(count);
            shift = 0;
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

/// Width, value and prefix geometry are checked during monomorphization.
///
/// ```compile_fail
/// let _ = oer_wire::U8::<4>::constant::<16>();
/// ```
/// ```compile_fail
/// let _ = oer_wire::U8::<0>::new(0);
/// ```
/// ```compile_fail
/// let _ = oer_wire::U8::<9>::from_low_bits(0);
/// ```
/// ```compile_fail
/// let _ = oer_wire::U16::<12>::ZERO.widen::<8>();
/// ```
/// ```compile_fail
/// let _ = oer_wire::PrefixLength::<4>::constant::<5>();
/// ```
/// ```compile_fail
/// let length = oer_wire::PrefixLength::<4>::constant::<3>();
/// let _ = length.borrow(&[0; 5]);
/// ```
/// ```compile_fail
/// let _ = oer_wire::Reader::new(&[0]).bits::<0>(0);
/// ```
/// ```compile_fail
/// let _ = oer_wire::Reader::new(&[0; 9]).bits::<65>(0);
/// ```
#[cfg(doctest)]
pub struct InvalidProofs;
