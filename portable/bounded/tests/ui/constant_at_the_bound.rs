#![no_std]
use oer_portable_bounded::Index;

#[cfg(not(ui_reject))]
pub const LAST: Index<4> = Index::constant::<3>();
// The bound itself is no index below it.
#[cfg(ui_reject)]
pub const LAST: Index<4> = Index::constant::<4>();
// ui-error@outside: E0080: evaluation panicked: the index lies inside the array
