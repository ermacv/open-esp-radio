//! Keep the large station and IP state machines in separate poll frames.
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
pub struct Boundary<'a, F>(pub Pin<&'a mut F>);
impl<F: Future> Future for Boundary<'_, F> {
    type Output = F::Output;
    #[inline(never)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}
