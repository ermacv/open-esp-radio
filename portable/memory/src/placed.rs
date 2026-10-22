//! Owned loans of values constructed in caller-provided storage.
//!
//! The backing cannot be reused while a projected field still owns it:
//! ```compile_fail,E0499
//! use core::mem::MaybeUninit;
//! use oer_memory::{Placed, init_in_place, unpack_placed};
//! struct Pair { first: u32, second: u32 }
//! let mut storage = MaybeUninit::uninit();
//! let pair = Placed::init(&mut storage, |p| init_in_place!(p, Pair { first: 1, second: 2 }));
//! let (first, second) = unpack_placed!(pair, Pair { first, second });
//! let another = Placed::init(&mut storage, |p| init_in_place!(p, Pair { first: 3, second: 4 }));
//! drop((first, second, another));
//! ```
//!
//! Projection cannot omit a user destructor:
//! ```compile_fail,E0509
//! use core::mem::MaybeUninit;
//! use oer_memory::{Placed, init_in_place, unpack_placed};
//! struct Owner { value: Box<u8> }
//! impl Drop for Owner { fn drop(&mut self) {} }
//! let mut storage = MaybeUninit::uninit();
//! let owner = Placed::init(&mut storage, |p| init_in_place!(p, Owner { value: Box::new(1) }));
//! let value = unpack_placed!(owner, Owner { value });
//! drop(value);
//! ```
#![allow(
    unsafe_code,
    reason = "the owner tracks initialization of borrowed storage"
)]

use core::{
    future::Future,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::{self, NonNull},
    task::{Context, Poll},
};

/// The sole destruction and extraction right for a placed value. Moving the
/// owner moves a pointer; its backing stays borrowed until this loan ends.
pub struct Placed<'a, T> {
    pointer: NonNull<T>,
    loan: PhantomData<&'a mut T>,
}

impl<'a, T> Placed<'a, T> {
    /// Initialize every field at its final address. The initializer must
    /// return the supplied destination, as checked before ownership transfers.
    pub fn init(
        storage: &'a mut MaybeUninit<T>,
        initialize: impl FnOnce(&mut MaybeUninit<T>) -> &mut T,
    ) -> Self {
        let destination = storage.as_mut_ptr();
        let value = initialize(storage);
        assert!(
            ptr::eq(value, destination),
            "initializer must use its destination"
        );
        Self {
            pointer: NonNull::from(value),
            loan: PhantomData,
        }
    }

    /// Extract a small owner. Large values use field projection or `move_to`.
    pub fn read(self) -> T {
        let (pointer, _) = self.__into_raw_parts();
        // SAFETY: consuming the loan transfers its sole initialized value.
        unsafe { pointer.as_ptr().read() }
    }

    /// Transfer ownership directly between borrowed places, without a T on
    /// the CPU stack. Neither place can overlap through exclusive safe loans.
    pub fn move_to<'b>(self, destination: &'b mut MaybeUninit<T>) -> Placed<'b, T> {
        let (pointer, _) = self.__into_raw_parts();
        // SAFETY: both places are exclusively borrowed, correctly aligned and
        // valid for T. The old loan was consumed and will never drop T again.
        unsafe { ptr::copy_nonoverlapping(pointer.as_ptr(), destination.as_mut_ptr(), 1) };
        Placed {
            pointer: NonNull::new(destination.as_mut_ptr()).expect("storage is non-null"),
            loan: PhantomData,
        }
    }

    /// Hand initialization to an enclosing placement initializer. That
    /// initializer assumes destruction responsibility for this field.
    pub fn into_mut(self) -> &'a mut T {
        let (mut pointer, _) = self.__into_raw_parts();
        // SAFETY: the consumed loan owned the only reference for 'a.
        unsafe { pointer.as_mut() }
    }

    #[doc(hidden)]
    pub fn __check(&self, _: impl FnOnce(T)) {}

    #[doc(hidden)]
    pub fn __into_raw_parts(self) -> (NonNull<T>, PlaceLoan<'a>) {
        let pointer = self.pointer;
        mem::forget(self);
        (pointer, PlaceLoan(PhantomData))
    }

    /// # Safety
    /// The pointer names one initialized, exclusively owned field of the
    /// consumed parent. Every field is projected exactly once; the parent has
    /// no Drop implementation. Only the placement macros establish this.
    #[doc(hidden)]
    pub unsafe fn __field(pointer: *mut T, _: &PlaceLoan<'a>) -> Self {
        Self {
            pointer: NonNull::new(pointer).expect("field is non-null"),
            loan: PhantomData,
        }
    }
}

#[doc(hidden)]
pub struct PlaceLoan<'a>(PhantomData<&'a mut ()>);

/// Construct array entries individually, cleaning an initialized prefix on
/// an early return or panic. No whole array is a temporary value.
pub fn init_array<T, const N: usize>(
    storage: &mut MaybeUninit<[T; N]>,
    mut entry: impl FnMut(usize) -> T,
) -> &mut [T; N] {
    struct Prefix<T> {
        pointer: *mut T,
        count: usize,
    }
    impl<T> Drop for Prefix<T> {
        fn drop(&mut self) {
            for index in 0..self.count {
                // SAFETY: exactly this initialized prefix belongs to us.
                unsafe { self.pointer.add(index).drop_in_place() };
            }
        }
    }
    let mut prefix = Prefix {
        pointer: storage.as_mut_ptr().cast::<T>(),
        count: 0,
    };
    for index in 0..N {
        // SAFETY: index is in bounds and its entry has not been initialized.
        unsafe { prefix.pointer.add(index).write(entry(index)) };
        prefix.count += 1;
    }
    mem::forget(prefix);
    // SAFETY: every array entry was initialized above.
    unsafe { storage.assume_init_mut() }
}

impl<T> Deref for Placed<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the loan owns initialized T and excludes other references.
        unsafe { self.pointer.as_ref() }
    }
}
impl<T> DerefMut for Placed<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the mutable loan is exclusive.
        unsafe { self.pointer.as_mut() }
    }
}
impl<T> Drop for Placed<'_, T> {
    fn drop(&mut self) {
        // SAFETY: this loan still owns T; extraction consumes and forgets it.
        unsafe { self.pointer.as_ptr().drop_in_place() }
    }
}

impl<'a, A, B> Placed<'a, (A, B)> {
    /// Split the pair's ownership without moving either value.
    pub fn split(self) -> (Placed<'a, A>, Placed<'a, B>) {
        let (pointer, loan) = self.__into_raw_parts();
        // SAFETY: distinct fields cover the pair, which has no Drop impl.
        unsafe {
            (
                Placed::__field(ptr::addr_of_mut!((*pointer.as_ptr()).0), &loan),
                Placed::__field(ptr::addr_of_mut!((*pointer.as_ptr()).1), &loan),
            )
        }
    }
}

/// Consume a named struct into field loans. Rust checks completeness,
/// visibility and the prohibition on moving fields from a Drop type.
#[macro_export]
macro_rules! unpack_placed {
    ($value:expr, $($ty:ident)::+ { $($field:ident),+ $(,)? }) => {{
        let __value = $value;
        __value.__check(|value| { let $($ty)::+ { $($field),+ } = value; drop(($($field),+)); });
        let (__pointer, __loan) = __value.__into_raw_parts();
        // SAFETY: the complete Rust-checked pattern names every field once.
        unsafe { ($($crate::Placed::__field($crate::__core::ptr::addr_of_mut!((*__pointer.as_ptr()).$field), &__loan)),+) }
    }};
}

/// Consume an enum into the selected variant's field loans. Every variant
/// and every field must be named; bodies receive owned `Placed` handles.
#[macro_export]
macro_rules! match_placed {
    ($value:expr, $ty:ident { $($variant:ident $fields:tt => $body:expr),+ $(,)? }) => {{
        let __value = $value;
        __value.__check(|value| match value {
            $($ty::$variant $fields => $crate::match_placed!(@check $fields)),+
        });
        let (mut __pointer, __loan) = __value.__into_raw_parts();
        // SAFETY: the consumed owner retains this initialized enum. The
        // checked exhaustive patterns project every field of its variant.
        match unsafe { __pointer.as_mut() } {
            $($ty::$variant $fields => $crate::match_placed!(@fields __loan; $fields; $body)),+
        }
    }};
    (@check ($($field:ident),* $(,)?)) => { drop(($($field,)*)) };
    (@check { $($field:ident),* $(,)? }) => { drop(($($field,)*)) };
    (@fields $loan:ident; ($($field:ident),* $(,)?); $body:expr) => {{
        // SAFETY: each reference names one distinct field of the variant.
        $(let $field = unsafe { $crate::Placed::__field($crate::__core::ptr::from_mut($field), &$loan) };)*
        $body
    }};
    (@fields $loan:ident; { $($field:ident),* $(,)? }; $body:expr) => {{
        // SAFETY: each reference names one distinct field of the variant.
        $(let $field = unsafe { $crate::Placed::__field($crate::__core::ptr::from_mut($field), &$loan) };)*
        $body
    }};
}

/// Poll a pinned future directly into caller-provided return storage. Its
/// output remains placed while the parent future handles only its loan.
pub fn poll_placed<'a, 'f, F: Future>(
    future: Pin<&'f mut F>,
    output: &'a mut MaybeUninit<Poll<F::Output>>,
) -> PollPlaced<'a, 'f, F> {
    PollPlaced {
        future,
        output: Some(output),
    }
}

pub struct PollPlaced<'a, 'f, F: Future> {
    future: Pin<&'f mut F>,
    output: Option<&'a mut MaybeUninit<Poll<F::Output>>>,
}
impl<'a, F: Future> Future for PollPlaced<'a, '_, F> {
    type Output = Placed<'a, F::Output>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let storage = this
            .output
            .take()
            .expect("a completed placed future is not polled again");
        // SAFETY: storage is exclusive and contains only a previous Pending
        // or uninitialized bytes. The callee's return place is this storage;
        // no MaybeUninit::write wrappers move the large output through locals.
        unsafe { storage.as_mut_ptr().write(this.future.as_mut().poll(cx)) };
        // SAFETY: the poll call initialized the complete value above.
        match unsafe { storage.assume_init_mut() } {
            Poll::Pending => {
                this.output = Some(storage);
                Poll::Pending
            }
            Poll::Ready(value) => {
                // SAFETY: Ready owns its one output, the outer Poll has no
                // destructor, and the loan retains the output storage.
                Poll::Ready(Placed {
                    pointer: NonNull::from(value),
                    loan: PhantomData,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
