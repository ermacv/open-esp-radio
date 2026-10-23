//! Typed poll boundary shared independently of the set of actor clients.
use core::future::Future;

/// A Wi-Fi-only image keeps its original future type and poll path.
#[cfg(not(feature = "bluetooth"))]
#[inline(always)]
pub(super) fn shared<F: Future>(future: F) -> F {
    future
}

/// Attached clients reuse one concrete physical poll implementation.
/// Inline construction into the caller's retained future; only polling forms
/// a boundary, so a large future needs no intermediate stack copy.
#[cfg(feature = "bluetooth")]
#[inline(always)]
pub(super) fn shared<F: Future>(future: F) -> impl Future<Output = F::Output> {
    Boundary { future }
}

#[cfg(feature = "bluetooth")]
pin_project_lite::pin_project! {
    struct Boundary<F> {
        #[pin]
        future: F,
    }
}
#[cfg(feature = "bluetooth")]
impl<F: Future> Future for Boundary<F> {
    type Output = F::Output;
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
