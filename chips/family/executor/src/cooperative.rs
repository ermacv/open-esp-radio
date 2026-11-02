//! The executor's cooperative admission attached to an image's timer.
use core::marker::PhantomData;

use oer_time::{Clock, Instant, Timer};

use crate::binding::Binding;

/// A timer with the ready-work and continuous-poll admission of the
/// executors of chip binding `B`. The wrapper retains only `T`; a
/// zero-sized timer remains zero-sized.
#[repr(transparent)]
pub struct Cooperative<B, T> {
    timer: T,
    binding: PhantomData<fn() -> B>,
}
impl<B, T> Cooperative<B, T> {
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            binding: PhantomData,
        }
    }
}
impl<B, T: Clone> Clone for Cooperative<B, T> {
    fn clone(&self) -> Self {
        Self::new(self.timer.clone())
    }
}
impl<B, T: Copy> Copy for Cooperative<B, T> {}
impl<B, T: Default> Default for Cooperative<B, T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
impl<B, T: core::fmt::Debug> core::fmt::Debug for Cooperative<B, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Cooperative").field(&self.timer).finish()
    }
}
impl<B, T: Clock> Clock for Cooperative<B, T> {
    fn now(&self) -> Instant {
        self.timer.now()
    }
}
impl<B, T: Timer> Timer for Cooperative<B, T> {
    fn wait_until(&self, at: Instant) -> impl core::future::Future<Output = ()> {
        self.timer.wait_until(at)
    }
}
impl<B: Binding, T: Timer> oer_cooperate::CooperativeTimer for Cooperative<B, T> {
    fn should_yield(&self) -> bool {
        crate::executor::should_yield::<B>()
    }
}
