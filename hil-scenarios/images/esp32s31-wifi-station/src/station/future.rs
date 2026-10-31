//! Keep the large station and IP state machines in separate poll frames, and
//! poll a future that may already have completed without resuming it.
use core::{
    cell::Cell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub(crate) struct Boundary<'a, F>(pub Pin<&'a mut F>);

impl<F: Future> Future for Boundary<'_, F> {
    type Output = F::Output;
    #[inline(never)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

/// Polls `future` until it completes and records that in `done`; once it
/// completed, it is ready without polling `future` again.
pub(crate) struct Tracked<'a, F> {
    pub future: Pin<&'a mut F>,
    pub done: &'a Cell<bool>,
}

impl<F: Future<Output = ()>> Future for Tracked<'_, F> {
    type Output = ();
    #[inline(never)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.done.get() {
            return Poll::Ready(());
        }
        let poll = self.future.as_mut().poll(cx);
        if poll.is_ready() {
            self.done.set(true);
        }
        poll
    }
}
