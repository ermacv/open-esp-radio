//! The executor's cooperative admission attached to an image's timer.
use oer_time::{Clock, Instant, Timer};

/// A timer with this executor's ready-work and continuous-poll admission.
/// The wrapper retains only `T`; a zero-sized timer remains zero-sized.
#[derive(Clone, Copy, Debug, Default)]
#[repr(transparent)]
pub struct Cooperative<T>(T);
impl<T> Cooperative<T> {
    pub const fn new(timer: T) -> Self {
        Self(timer)
    }
}
impl<T: Clock> Clock for Cooperative<T> {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl<T: Timer> Timer for Cooperative<T> {
    fn wait_until(&self, at: Instant) -> impl core::future::Future<Output = ()> {
        self.0.wait_until(at)
    }
}
impl<T: Timer> oer_cooperate::CooperativeTimer for Cooperative<T> {
    fn should_yield(&self) -> bool {
        crate::executor::should_yield()
    }
}
