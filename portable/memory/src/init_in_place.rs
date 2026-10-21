//! Construct complete named structs without moving the whole value.

/// Initialize every field of a named struct directly in `MaybeUninit` storage.
///
/// The destination is evaluated once, followed by the field expressions in
/// source order. Every field must occur exactly once. If an expression panics
/// or returns early, fields already initialized are dropped. The returned
/// reference borrows the destination. This macro does not pin the value.
///
/// ```
/// #![forbid(unsafe_code)]
/// use core::mem::MaybeUninit;
/// struct Pair { first: u32, second: u32 }
/// let mut storage = MaybeUninit::uninit();
/// let pair = oer_memory::init_in_place!(&mut storage, Pair {
///     first: 12,
///     second: 34,
/// });
/// assert_eq!((pair.first, pair.second), (12, 34));
/// ```
#[macro_export]
macro_rules! init_in_place {
    ($storage:expr, $($ty:ident)::+ { $($field:ident: $value:expr),+ $(,)? }) => {{
        let __storage: &mut core::mem::MaybeUninit<$($ty)::+> = $storage;
        // This never runs or captures inputs. Rust checks the complete
        // initializer, including field visibility and construction safety.
        let _ = |$($field),+| $($ty)::+ { $($field),+ };
        let __pointer = __storage.as_mut_ptr();
        let __fields = ($({
            // Keep caller expressions outside this macro's unsafe context.
            let __value = $value;
            // SAFETY: the destination is exclusively borrowed and valid for
            // the struct. This field exists and is written exactly once.
            // Raw, unaligned access also admits repr(packed) structs.
            unsafe {
                let __field = core::ptr::addr_of_mut!((*__pointer).$field);
                __field.write_unaligned(__value);
                $crate::__InitField::new(__field)
            }
        },)+);
        // All fields now belong to the complete value, not the cleanup guards.
        core::mem::forget(__fields);
        // SAFETY: the checked initializer includes every field. Each was
        // initialized above, with no reference to uninitialized fields.
        unsafe { __storage.assume_init_mut() }
    }};
}

/// Cleanup for one initialized field of an incomplete struct.
#[doc(hidden)]
pub struct __InitField<T>(*mut T);

impl<T> __InitField<T> {
    /// Retain the exclusive right to drop one initialized field.
    ///
    /// # Safety
    ///
    /// `pointer` must address a valid initialized `T`, exclusively owned by
    /// this guard until it is dropped or forgotten. Moving `T` out of this
    /// place must be permitted; it must not be pinned. It may be unaligned.
    #[allow(unsafe_code, reason = "only the placement macro creates field guards")]
    pub unsafe fn new(pointer: *mut T) -> Self {
        Self(pointer)
    }
}

impl<T> Drop for __InitField<T> {
    #[allow(unsafe_code, reason = "the guard owns this initialized field")]
    fn drop(&mut self) {
        // SAFETY: the constructor's contract retains this initialized field
        // exclusively. Reading unaligned supports packed structs; the local
        // value is aligned when its destructor runs. No full value is exposed
        // after incomplete initialization, so the field is not dropped again.
        drop(unsafe { self.0.read_unaligned() });
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::{cell::Cell, mem::MaybeUninit};
    use std::{cell::RefCell, panic::AssertUnwindSafe, rc::Rc, vec, vec::Vec};

    struct Entry {
        id: u8,
        events: Rc<RefCell<Vec<u8>>>,
    }

    impl Drop for Entry {
        fn drop(&mut self) {
            self.events.borrow_mut().push(self.id + 10);
        }
    }

    struct Pair {
        first: Entry,
        second: Entry,
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            self.first.events.borrow_mut().push(100);
        }
    }

    fn entry(id: u8, events: &Rc<RefCell<Vec<u8>>>) -> Entry {
        events.borrow_mut().push(id);
        Entry {
            id,
            events: events.clone(),
        }
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "exercise this crate's audited macro and initialized storage"
    )]
    fn complete_value_keeps_its_fields_until_dropped() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let destinations = Cell::new(0);
        let mut storage = MaybeUninit::uninit();
        let pair = crate::init_in_place!(
            {
                destinations.set(destinations.get() + 1);
                &mut storage
            },
            Pair {
                second: entry(2, &events),
                first: entry(1, &events)
            }
        );
        assert_eq!((pair.first.id, pair.second.id), (1, 2));
        assert_eq!(destinations.get(), 1);
        assert_eq!(*events.borrow(), vec![2, 1]);
        // SAFETY: the macro initialized every field and its returned loan ended.
        drop(unsafe { storage.assume_init() });
        assert_eq!(*events.borrow(), vec![2, 1, 100, 11, 12]);
    }

    fn fail() -> Entry {
        panic!("interrupt field construction")
    }

    #[test]
    #[allow(unsafe_code, reason = "exercise this crate's audited placement macro")]
    fn panic_drops_only_completed_fields() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut storage = MaybeUninit::uninit();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = crate::init_in_place!(
                &mut storage,
                Pair {
                    first: entry(1, &events),
                    second: fail(),
                }
            );
        }));
        assert!(result.is_err());
        assert_eq!(*events.borrow(), vec![1, 11]);
    }

    #[allow(unsafe_code, reason = "exercise this crate's audited placement macro")]
    fn interrupted(
        storage: &mut MaybeUninit<Pair>,
        events: &Rc<RefCell<Vec<u8>>>,
    ) -> Result<(), ()> {
        let _ = crate::init_in_place!(
            storage,
            Pair {
                first: entry(1, events),
                second: Err::<Entry, ()>(())?,
            }
        );
        Ok(())
    }

    #[test]
    fn early_return_drops_completed_fields() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut storage = MaybeUninit::uninit();
        assert!(interrupted(&mut storage, &events).is_err());
        assert_eq!(*events.borrow(), vec![1, 11]);
    }

    #[repr(C, packed)]
    struct Packed {
        tag: u8,
        item: Entry,
    }

    fn fail_tag() -> u8 {
        panic!("interrupt packed construction")
    }

    #[test]
    #[allow(unsafe_code, reason = "exercise this crate's audited placement macro")]
    fn panic_can_drop_an_unaligned_field() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut storage = MaybeUninit::uninit();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = crate::init_in_place!(
                &mut storage,
                Packed {
                    item: entry(1, &events),
                    tag: fail_tag(),
                }
            );
        }));
        assert!(result.is_err());
        assert_eq!(*events.borrow(), vec![1, 11]);
    }
}
