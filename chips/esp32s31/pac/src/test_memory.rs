//! Synthetic layout references for transaction recorders; never dereferenced.

use crate::layouts::{DmaLayout, DmaRef, DmaWindowAddress};

#[allow(
    unsafe_code,
    clippy::disallowed_methods,
    reason = "host transaction recorders use synthetic addresses without accessing memory"
)]
pub(crate) const fn reference<L: DmaLayout>(address: u32) -> DmaRef<L> {
    // SAFETY: the host models record pointer identities only. They never
    // dereference these synthetic references or publish them to hardware.
    unsafe { DmaRef::new_unchecked(DmaWindowAddress::new_unchecked(address)) }
}
