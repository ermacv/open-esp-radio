//! The image's table, adopted once at boot and read by every core.

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

/// A `&'static` table one adopter stores once and every core reads.
pub struct Adopted<T: 'static> {
    claimed: AtomicBool,
    first: AtomicPtr<T>,
    length: AtomicUsize,
}

/// A second [`Adopted::adopt`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlreadyAdopted;

impl<T: Sync + 'static> Adopted<T> {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            first: AtomicPtr::new(core::ptr::null_mut()),
            length: AtomicUsize::new(0),
        }
    }

    /// Store `table` for every reader.
    ///
    /// # Errors
    ///
    /// [`AlreadyAdopted`] once a table is stored; the stored one stays.
    pub fn adopt(&self, table: &'static [T]) -> Result<(), AlreadyAdopted> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            return Err(AlreadyAdopted);
        }
        self.length.store(table.len(), Ordering::Relaxed);
        self.first
            .store(table.as_ptr().cast_mut(), Ordering::Release);
        Ok(())
    }

    /// The adopted table; `None` before [`Adopted::adopt`].
    pub fn get(&self) -> Option<&'static [T]> {
        let first = self.first.load(Ordering::Acquire);
        if first.is_null() {
            return None;
        }
        // SAFETY: `adopt` stored, once, the length and then the start of a
        // `&'static [T]`; the acquire load of the start orders the length's
        // load after its store, and `T: Sync` lets every core share it.
        #[allow(unsafe_code, reason = "rebuilding the one adopted `&'static` slice")]
        Some(unsafe { core::slice::from_raw_parts(first, self.length.load(Ordering::Relaxed)) })
    }
}

impl<T: Sync + 'static> Default for Adopted<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_is_adopted_once_and_absent_before() {
        static WORDS: [u32; 3] = [1, 2, 3];
        static OTHER: [u32; 1] = [9];
        let adopted = Adopted::<u32>::new();
        assert_eq!(adopted.get(), None);
        adopted.adopt(&WORDS).unwrap();
        assert_eq!(adopted.get(), Some(&WORDS[..]));
        assert_eq!(adopted.adopt(&OTHER), Err(AlreadyAdopted));
        assert_eq!(adopted.get(), Some(&WORDS[..]));
    }

    #[test]
    fn an_empty_table_is_adopted_not_absent() {
        static NONE: [u32; 0] = [];
        let adopted = Adopted::<u32>::new();
        adopted.adopt(&NONE).unwrap();
        assert_eq!(adopted.get(), Some(&[][..]));
    }
}
