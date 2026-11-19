#![no_std]
use oer_portable_mmio::check::disjoint_blocks;

// The pending bytes of two lines, held by a shared block.
const PENDING: (usize, &[usize], &[usize]) = (0x1000, &[], &[0, 4]);
#[cfg(not(ui_reject))]
const OTHER: (usize, &[usize], &[usize]) = (0x1000, &[], &[1, 5]);
// Another block's claim of a byte the shared block claims.
#[cfg(ui_reject)]
const OTHER: (usize, &[usize], &[usize]) = (0x1000, &[], &[1, 4]);

const _: () = if !disjoint_blocks(&[PENDING, OTHER]) {
    panic!("no two claimed blocks list the same place") // ui-error: E0080
};
