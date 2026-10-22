//! Construct complete named structs without moving the whole value.

/// Initialize every field of a named struct directly in `MaybeUninit` storage.
///
/// The destination is evaluated once, followed by the field expressions in
/// source order. Every field must occur exactly once. If an expression panics
/// or returns early, fields already initialized are dropped. The returned
/// reference borrows the destination. This macro does not pin the value.
/// A `field => initializer` receives that field's uninitialized storage and
/// must return a valid reference to the same field. This permits nested
/// placement without constructing a complete field on the stack. Completed
/// fields are cleaned up if a later initializer fails.
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
    ($storage:expr, $($ty:ident)::+ { $($field:ident $kind:tt $value:expr),+ $(,)? }) => {{
        let __storage: &mut $crate::__core::mem::MaybeUninit<$($ty)::+> = $storage;
        // This never runs or captures inputs. Rust checks the complete
        // initializer, including field visibility and construction safety.
        let _ = |$($field),+| $($ty)::+ { $($field),+ };
        let __pointer = __storage.as_mut_ptr();
        let __fields = ($($crate::init_in_place!(@field __pointer, $field $kind $value),)+);
        // All fields now belong to the complete value, not the cleanup guards.
        $crate::__core::mem::forget(__fields);
        // SAFETY: the checked initializer includes every field. Each was
        // initialized above, with no reference to uninitialized fields.
        unsafe { __storage.assume_init_mut() }
    }};
    (@field $pointer:ident, $field:ident : $value:expr) => {{
        // Keep caller expressions outside this macro's unsafe context.
        let __value = $value;
        // SAFETY: the destination is exclusively borrowed and valid for
        // the struct. This field exists and is written exactly once.
        // Raw, unaligned access also admits repr(packed) structs.
        unsafe {
            let __field = $crate::__core::ptr::addr_of_mut!((*$pointer).$field);
            __field.write_unaligned(__value);
            $crate::__InitField::new(__field)
        }
    }};
    (@field $pointer:ident, $field:ident => $initializer:expr) => {{
        let __initialize = $crate::__InitField::initializer($initializer);
        // SAFETY: the checked struct initializer proves the field exists;
        // its storage is exclusively borrowed and has not been initialized.
        // initialize checks alignment and the returned reference's address
        // before granting the initialized-field destruction guard.
        unsafe {
            let __field = $crate::__core::ptr::addr_of_mut!((*$pointer).$field);
            $crate::__InitField::initialize(__field, __initialize)
        }
    }};
}

/// Cleanup for one initialized field of an incomplete struct.
#[doc(hidden)]
pub struct __InitField<T>(*mut T);

impl<T> __InitField<T> {
    // Infer a closure against the higher-ranked signature before moving it
    // into the macro's unsafe field-projection block.
    #[inline(always)]
    pub fn initializer<F>(initialize: F) -> F
    where
        F: for<'a> FnOnce(&'a mut core::mem::MaybeUninit<T>) -> &'a mut T,
    {
        initialize
    }

    /// Initialize an exclusively owned field and retain its destruction right.
    ///
    /// # Safety
    /// `pointer` must address one exclusively borrowed, uninitialized field
    /// valid for `T`. It need not be aligned; misalignment is rejected before
    /// a reference is created. The field must not be pinned.
    #[inline(always)]
    #[allow(unsafe_code, reason = "the placement macro owns this field's storage")]
    pub unsafe fn initialize(
        pointer: *mut T,
        initialize: impl for<'a> FnOnce(&'a mut core::mem::MaybeUninit<T>) -> &'a mut T,
    ) -> Self {
        assert!(
            pointer.is_aligned(),
            "nested placement requires an aligned field"
        );
        // SAFETY: the caller grants exclusive storage for the field. Every
        // bit pattern is valid for MaybeUninit<T>, and alignment was checked.
        let storage = unsafe { &mut *pointer.cast::<core::mem::MaybeUninit<T>>() };
        let initialized = initialize(storage);
        assert!(
            core::ptr::eq(initialized, pointer),
            "nested initializer must initialize its destination"
        );
        // A valid T reference at this exact address proves initialization.
        // No caller can access the exclusive storage while this guard lives.
        Self(pointer)
    }

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

    struct Nested {
        item: Entry,
    }
    struct Outer {
        first: Entry,
        nested: Nested,
        last: Entry,
    }

    #[test]
    #[allow(unsafe_code, reason = "exercise audited nested placement")]
    fn nested_fields_keep_their_final_address_and_drop_once() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut storage = MaybeUninit::<Outer>::uninit();
        let base = storage.as_mut_ptr();
        let outer = crate::init_in_place!(&mut storage, Outer {
            first: entry(1, &events),
            nested => |place| crate::init_in_place!(place, Nested { item: entry(2, &events) }),
            last: entry(3, &events),
        });
        assert!(core::ptr::eq(outer, base));
        assert_eq!(
            (outer.first.id, outer.nested.item.id, outer.last.id),
            (1, 2, 3)
        );
        assert_eq!(*events.borrow(), vec![1, 2, 3]);
        // SAFETY: the complete initializer succeeded and its borrow ended;
        // this test owns the value's one destruction right.
        unsafe { storage.assume_init_drop() };
        assert_eq!(*events.borrow(), vec![1, 2, 3, 11, 12, 13]);
    }

    #[test]
    #[allow(unsafe_code, reason = "exercise audited nested cleanup")]
    fn failure_after_nested_placement_drops_completed_owners_once() {
        fn interrupted() -> Entry {
            panic!("interrupt after nested placement")
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut storage = MaybeUninit::<Outer>::uninit();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = crate::init_in_place!(&mut storage, Outer {
                first: entry(1, &events),
                nested => |place| crate::init_in_place!(place, Nested { item: entry(2, &events) }),
                last: interrupted(),
            });
        }));
        assert!(result.is_err());
        let events = events.borrow();
        assert_eq!(&events[..2], [1, 2]);
        let mut destroyed = events[2..].to_vec();
        destroyed.sort();
        assert_eq!(destroyed, [11, 12]);
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "reject a nested initializer returning a different allocation"
    )]
    fn a_reference_to_another_value_cannot_claim_initialization() {
        fn elsewhere(_: &mut MaybeUninit<u32>) -> &mut u32 {
            std::boxed::Box::leak(std::boxed::Box::new(7))
        }
        struct Value {
            field: u32,
        }
        let mut storage = MaybeUninit::<Value>::uninit();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = crate::init_in_place!(&mut storage, Value { field => elsewhere });
        }));
        assert!(result.is_err());
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "reject an unaligned nested field before creating a reference"
    )]
    fn an_unaligned_nested_field_is_rejected_before_calling_its_initializer() {
        #[repr(C, packed)]
        struct Unaligned {
            tag: u8,
            number: u32,
        }
        #[repr(align(64))]
        struct Aligned(MaybeUninit<Unaligned>);
        fn initialize(storage: &mut MaybeUninit<u32>) -> &mut u32 {
            storage.write(7)
        }
        let mut storage = Aligned(MaybeUninit::uninit());
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ =
                crate::init_in_place!(&mut storage.0, Unaligned { tag: 1, number => initialize });
        }));
        assert!(result.is_err());
    }
}
