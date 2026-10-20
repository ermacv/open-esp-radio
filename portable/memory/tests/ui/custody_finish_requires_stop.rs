#![no_std]
use oer_memory::{CustodyDomain, Drained, IndexedCustody, Quarantine};

// The ordinary end needs the binding's own stop proof; no other value can
// stand in for "hardware stopped".
#[cfg(not(ui_reject))]
pub fn finish<T: Quarantine, D: CustodyDomain<N>, const N: usize>(
    storage: IndexedCustody<T, D, N>,
    stopped: &D::Stopped,
) -> Drained<T, N> {
    storage.finish(stopped)
}

#[cfg(ui_reject)]
pub fn finish<T: Quarantine, D: CustodyDomain<N>, const N: usize>(
    storage: IndexedCustody<T, D, N>,
    stopped: &(),
) -> Drained<T, N> {
    storage.finish(stopped) // ui-error: E0308
}
