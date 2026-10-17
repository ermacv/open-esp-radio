//! Explicit poll boundary for large statically stored async state machines.
//!
//! Embassy task futures live in static task arenas, but fat LTO may inline a
//! complete child `Future::poll` into its parent and accumulate every child's
//! local allocas in one live CPU stack frame. This adapter borrows an already
//! pinned child future, so it changes only the compiler call boundary: it does
//! not move the child future, allocate, or alter ownership and cancellation.

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[must_use = "futures do nothing unless polled"]
pub struct StackPoll<'future, F> {
    future: Pin<&'future mut F>,
}

impl<'future, F> StackPoll<'future, F> {
    pub const fn new(future: Pin<&'future mut F>) -> Self {
        Self { future }
    }
}

impl<F: Future> Future for StackPoll<'_, F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        type PollFn<F> = for<'future, 'context, 'wake> fn(
            Pin<&'future mut F>,
            &'context mut Context<'wake>,
        ) -> Poll<<F as Future>::Output>;

        let this = self.get_mut();
        let poll: PollFn<F> = F::poll;
        // A direct generic call is devirtualized and re-inlined through fat
        // LTO. Keeping the target opaque is the actual compiler boundary.
        core::hint::black_box(poll)(this.future.as_mut(), context)
    }
}

pub const fn stack_poll<F: Future>(future: Pin<&mut F>) -> StackPoll<'_, F> {
    StackPoll::new(future)
}

/// Pin a child future in its parent's async state and poll it through a real
/// call boundary. The wrapper stores only a pinned reference, never the large
/// future or its owner-bearing output.
#[macro_export]
macro_rules! await_stack_boundary {
    ($future:expr $(,)?) => {{
        let mut future = core::pin::pin!($future);
        $crate::stack_boundary::stack_poll(future.as_mut()).await
    }};
}
