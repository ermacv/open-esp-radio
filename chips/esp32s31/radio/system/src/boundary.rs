//! Typed poll boundary shared independently of the set of actor clients.
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
pin_project_lite::pin_project! {
    pub(super) struct Boundary<F> {
        #[pin]
        future:F,
    }
}
impl<F> Boundary<F> {
    pub(super) fn new(future: F) -> Self {
        Self { future }
    }
}
impl<F: Future> Future for Boundary<F> {
    type Output = F::Output;
    // Multiple client paths share one physical poll body. A Wi-Fi-only
    // image has one caller and can inline the identical typed projection.
    #[cfg_attr(feature = "bluetooth", inline(never))]
    #[cfg_attr(not(feature = "bluetooth"), inline(always))]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
