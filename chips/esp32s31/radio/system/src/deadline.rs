//! The optional tracking deadline shares one timer shape.
use core::future::Future;

// A missing deadline is a permanently pending timer, so both wait modes use
// one select state. Construct a real timer before polling either contender.
#[cfg(any(feature = "bluetooth", test))]
pin_project_lite::pin_project! {
    pub(super) struct Deadline<F> {
        #[pin]
        future: Option<F>,
    }
}
#[cfg(any(feature = "bluetooth", test))]
impl<F> Deadline<F> {
    #[inline(always)]
    pub(super) fn new(future: Option<F>) -> Self {
        Self { future }
    }
}
#[cfg(any(feature = "bluetooth", test))]
impl<F: Future<Output = ()>> Future for Deadline<F> {
    type Output = ();
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        match self.project().future.as_pin_mut() {
            Some(future) => future.poll(cx),
            None => core::task::Poll::Pending,
        }
    }
}
