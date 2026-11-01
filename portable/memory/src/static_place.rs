//! One-shot static backing for values carrying shorter body loans.
#![allow(
    unsafe_code,
    reason = "static backing is claimed permanently before its type is lent"
)]

use core::{
    future::Future,
    marker::PhantomData,
    mem::{MaybeUninit, align_of, size_of},
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

/// Uninitialized backing whose size and alignment come from `Layout`.
///
/// A `ConstStaticCell<StaticPlace<Value<'static>>>` can back `Value<'body>` without
/// extending any reference's lifetime. The backing contains no initialized
/// `Layout`, and each claim permanently consumes its exclusive static loan.
/// The value's actual type is checked against the backing at monomorphization.
/// Use separate places for values that coexist, including a future and its
/// output. There is no reset or reuse operation.
pub struct StaticPlace<Layout> {
    backing: MaybeUninit<Layout>,
}

// SAFETY: an owned StaticPlace contains only uninitialized bytes, never a
// Layout. Every operation that can initialize them permanently borrows self
// for 'static, so the backing cannot move afterward. The initialized handle
// retains its actual type's thread restrictions independently of Layout.
unsafe impl<Layout> Send for StaticPlace<Layout> {}

impl<Layout: 'static> StaticPlace<Layout> {
    pub const fn new() -> Self {
        Self {
            backing: MaybeUninit::uninit(),
        }
    }

    /// Claim storage for one value, retaining its actual, possibly short,
    /// lifetime. Initialization and destruction belong to the caller.
    ///
    /// Stack backing cannot grant the permanent loan:
    /// ```compile_fail,E0597
    /// use oer_memory::StaticPlace;
    /// let mut place = StaticPlace::<u32>::new();
    /// let _ = place.uninit::<u32>();
    /// ```
    /// Nor can a static place be claimed twice:
    /// ```compile_fail,E0499
    /// use oer_memory::StaticPlace;
    /// let place = Box::leak(Box::new(StaticPlace::<u32>::new()));
    /// let first = place.uninit::<u32>();
    /// let second = place.uninit::<u32>();
    /// let _ = (first, second);
    /// ```
    pub fn uninit<'a, T>(&'static mut self) -> &'a mut MaybeUninit<T> {
        const {
            assert!(
                size_of::<T>() <= size_of::<Layout>(),
                "static place is too small"
            );
            assert!(
                align_of::<T>() <= align_of::<Layout>(),
                "static place is under-aligned"
            );
        }
        // SAFETY: the permanent exclusive loan prevents reuse, even if the
        // initialized owner is forgotten. The allocation outlives 'a; only
        // uninitialized bytes are reinterpreted, with checked size/alignment.
        unsafe { &mut *self.backing.as_mut_ptr().cast::<MaybeUninit<T>>() }
    }

    /// Construct a future at its final address and pin it once. The handle
    /// owns destruction, including cancellation before the first poll.
    ///
    /// Never inlined: a future the initializer returns by value may be
    /// assembled in this call's frame before it is written, and that frame
    /// ends before the future's first poll. Inlined into an async caller's
    /// poll, it stayed on the stack under every poll of the future.
    #[inline(never)]
    pub fn future<'a, F: Future + 'a>(
        &'static mut self,
        initialize: impl FnOnce() -> F,
    ) -> PlacedFuture<'a, F> {
        let storage = self.uninit::<F>();
        // SAFETY: the initializer writes the claimed static return place.
        // No reference to the uninitialized future is formed.
        unsafe { storage.as_mut_ptr().write(initialize()) };
        PlacedFuture {
            pointer: NonNull::from(storage).cast(),
            loan: PhantomData,
        }
    }
}

impl<Layout: 'static> Default for StaticPlace<Layout> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sole polling and destruction right for a future pinned in static backing.
/// Moving this handle leaves the future at its initialized address.
pub struct PlacedFuture<'a, F> {
    pointer: NonNull<F>,
    loan: PhantomData<&'a mut F>,
}

impl<F: Future> Future for PlacedFuture<'_, F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: the handle owns the permanently reserved static place.
        // Its API exposes neither a movable F nor the backing. Drop destroys
        // F in place; forgetting the handle retains its backing forever.
        unsafe { Pin::new_unchecked(&mut *self.pointer.as_ptr()).poll(cx) }
    }
}

impl<F> Drop for PlacedFuture<'_, F> {
    // Keep a large future's state-dependent cleanup out of its executor's
    // task poll, just as polling itself stays behind the boundary above.
    #[inline(never)]
    fn drop(&mut self) {
        // SAFETY: this sole owner destroys the initialized future at its
        // pinned address before its borrowed inputs may expire.
        unsafe { self.pointer.as_ptr().drop_in_place() };
    }
}

#[cfg(test)]
mod tests;
