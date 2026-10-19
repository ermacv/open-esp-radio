//! Time arithmetic that never saturates or substitutes a value: an
//! overflow is a typed error the caller reports.

use core::fmt;

/// A sum or product of microseconds that does not fit in `u64`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Overflow;

impl fmt::Display for Overflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a time does not fit in 64-bit microseconds")
    }
}

impl std::error::Error for Overflow {}

pub(crate) fn add(a: u64, b: u64) -> Result<u64, Overflow> {
    a.checked_add(b).ok_or(Overflow)
}

pub(crate) fn mul(a: u64, b: u64) -> Result<u64, Overflow> {
    a.checked_mul(b).ok_or(Overflow)
}

/// The sum of `values`, or the overflow.
pub fn sum(values: impl IntoIterator<Item = u64>) -> Result<u64, Overflow> {
    values.into_iter().try_fold(0, add)
}
