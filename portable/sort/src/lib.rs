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

//! In-place sorting without recursion, allocation or a pivot: the standard
//! library's unstable sort recurses, which no stack bound can follow. The
//! heap sort here takes `O(n log n)` time for every input, in a fixed frame.

use core::mem;

/// Sorts `values` in ascending order. The order of equal values is not kept.
pub fn sort<T: Ord>(values: &mut [T]) {
    let length = values.len();
    for root in (0..length / 2).rev() {
        sift_down(values, root);
    }
    let mut heap = values;
    while let Some((last, remaining)) = heap.split_last_mut() {
        let Some((root, _)) = remaining.split_first_mut() else {
            return;
        };
        mem::swap(root, last);
        sift_down(remaining, 0);
        heap = remaining;
    }
}

/// Restores the max-heap below `root` within `values`.
fn sift_down<T: Ord>(values: &mut [T], root: usize) {
    let mut nodes = values.iter_mut().enumerate().peekable();
    let Some((mut index, mut parent)) = nodes.nth(root) else {
        return;
    };
    // After taking node r, the iterator starts at r + 1. Skipping r nodes
    // reaches its left child without calculating the possibly overflowing
    // 2 * r + 1. Slice-iterator nth skips in constant time, even for ZSTs.
    while let Some((left_index, left)) = nodes.nth(index) {
        // Leave the right sibling in the iterator when selecting the left:
        // the next iteration must again start immediately after its parent.
        let (child_index, child) = match nodes.next_if(|(_, right)| *left < **right) {
            Some(right) => right,
            None => (left_index, left),
        };
        if *parent >= *child {
            return;
        }
        mem::swap(parent, child);
        index = child_index;
        parent = child;
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "tests use assertions, standard-library oracles and bounded input generation"
)]
mod tests;
