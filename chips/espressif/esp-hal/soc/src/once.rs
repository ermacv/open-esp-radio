//! A flag that grants one first claim per program, for a transfer that has
//! no esp-hal singleton to consume.

use core::sync::atomic::{AtomicBool, Ordering};

/// Set by the first claim; every later claim sees it set.
pub(crate) struct Once(AtomicBool);

impl Once {
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Whether this is the first claim; it sets the flag.
    pub(crate) fn first(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::Once;

    #[test]
    fn only_the_first_claim_succeeds() {
        let once = Once::new();
        assert!(once.first());
        assert!(!once.first());
        assert!(!once.first());
    }
}
