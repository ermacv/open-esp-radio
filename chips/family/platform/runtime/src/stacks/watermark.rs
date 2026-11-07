//! Paint and scan only an exclusively owned, inactive stack allocation.

const PAINT: u32 = 0xa55a_a55a;

/// # Safety
/// `bottom` must name `words` writable aligned words, exclusively owned by the
/// caller and not in use as a stack. Paint only before initial stack admission.
pub unsafe fn paint(bottom: *mut u32, words: usize) {
    for index in 0..words {
        // SAFETY: `index < words`, which the caller's contract makes
        // writable, aligned and exclusively ours.
        unsafe { bottom.add(index).write_volatile(PAINT) };
    }
}

/// # Safety
/// The range must be initialized, readable and excluded from concurrent writes.
/// This counts the untouched prefix; it does not infer usage above a paint hole.
pub unsafe fn free_words(bottom: *const u32, words: usize) -> usize {
    for index in 0..words {
        // SAFETY: `index < words`, which the caller's contract makes
        // initialized and readable.
        if unsafe { bottom.add(index).read_volatile() } != PAINT {
            return index;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The free words of `stack`, a test's own initialized array.
    fn free(stack: &[u32]) -> usize {
        // SAFETY: `stack` is initialized, readable and borrowed shared for
        // the scan, so nothing writes it meanwhile.
        unsafe { free_words(stack.as_ptr(), stack.len()) }
    }

    #[test]
    fn paints_and_tracks_deeper_writes_without_repainting_or_skipping_holes() {
        let mut stack = [0; 16];
        // SAFETY: the test owns `stack`, which is no stack in use.
        unsafe { paint(stack.as_mut_ptr(), stack.len()) };
        assert_eq!(free(&stack), 16);
        stack[12] = 7;
        assert_eq!(free(&stack), 12);
        stack[5] = 9;
        assert_eq!(free(&stack), 5);
        stack[12] = PAINT;
        assert_eq!(free(&stack), 5);
        stack[0] = 1;
        assert_eq!(free(&stack), 0);
    }

    #[test]
    fn range_limits_exclude_neighbor_storage() {
        let mut storage = [99; 6];
        // SAFETY: words 1 to 4 of the test's own `storage`, no stack in use.
        unsafe { paint(storage.as_mut_ptr().add(1), 4) };
        assert_eq!(storage[0], 99);
        assert_eq!(storage[5], 99);
        assert_eq!(free(&storage[1..5]), 4);
    }
}
