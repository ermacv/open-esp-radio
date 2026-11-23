#![no_std]
use oer_portable_bounded::Index;

#[cfg(not(ui_reject))]
pub const BYTE: u8 = Index::<256>::constant::<255>().to_u8();
// A bound above 256 holds numbers no byte holds.
#[cfg(ui_reject)]
pub const BYTE: u8 = Index::<257>::constant::<255>().to_u8();
// ui-error@outside: E0080: evaluation panicked: every number below the bound is a byte
