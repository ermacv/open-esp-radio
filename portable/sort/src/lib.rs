#![no_std]
#![forbid(unsafe_code)]

//! In-place sorting without recursion, allocation or a pivot: the standard
//! library's unstable sort recurses, which no stack bound can follow. The
//! heap sort here takes `O(n log n)` time for every input, in a fixed frame.

/// Sorts `values` in ascending order. The order of equal values is not kept.
pub fn sort<T: Ord>(values: &mut [T]) {
    let length = values.len();
    for root in (0..length / 2).rev() {
        sift_down(values, root, length);
    }
    for end in (1..length).rev() {
        values.swap(0, end);
        sift_down(values, 0, end);
    }
}

/// Restores the max-heap below `root` within `values[..end]`.
fn sift_down<T: Ord>(values: &mut [T], mut root: usize, end: usize) {
    loop {
        let mut child = 2 * root + 1;
        if child >= end {
            return;
        }
        if child + 1 < end && values.get(child) < values.get(child + 1) {
            child += 1;
        }
        if values.get(root) >= values.get(child) {
            return;
        }
        values.swap(root, child);
        root = child;
    }
}

#[cfg(test)]
mod tests;
