//! A poll boundary for PHY work and its enclosing radio actor.
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pin_project_lite::pin_project! {
    /// Retain a future in its caller's storage and poll it out of line.
    pub struct Boundary<F> {
        #[pin]
        future: F,
    }
}

impl<F: Future> Boundary<F> {
    /// Construct directly in the caller's return place.
    #[inline(always)]
    pub fn new(future: F) -> Self {
        Self { future }
    }
}

impl<F: Future> Future for Boundary<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
