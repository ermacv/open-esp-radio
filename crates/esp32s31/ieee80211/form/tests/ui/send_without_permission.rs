#![no_std]
use oer_memory::{AffineIndexedSpscSender, UniqueQueueIndex};

// The positive twin sends the whole affine permission owner. Removing its
// audited index bound makes the infallible publication API unavailable.
#[cfg(not(ui_reject))]
pub fn publish<T: UniqueQueueIndex<N>, const N: usize>(
    sender: &mut AffineIndexedSpscSender<'_, T, N>,
    permission: T,
) {
    sender.send(permission);
}
#[cfg(ui_reject)]
pub fn publish<T, const N: usize>(
    sender: &mut AffineIndexedSpscSender<'_, T, N>,
    value: T,
) {
    sender.send(value); // ui-error: E0599
}
