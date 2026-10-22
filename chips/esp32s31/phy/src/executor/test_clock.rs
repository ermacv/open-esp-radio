//! Each test owns its virtual clock and synthetic peer readiness.
use core::{cell::Cell, ops::Deref};
use oer_time::{Clock, Instant, Timer};

pub(crate) struct VirtualClock<const N: usize = 8> {
    clock: oer_time_virtual::VirtualClock<N>,
    peers: Cell<bool>,
    peer_at: Cell<Option<Instant>>,
}
impl<const N: usize> VirtualClock<N> {
    pub(crate) fn new() -> Self {
        Self::starting_at(Instant::EPOCH)
    }
    pub(crate) fn starting_at(at: Instant) -> Self {
        Self {
            clock: oer_time_virtual::VirtualClock::starting_at(at),
            peers: Cell::new(true),
            peer_at: Cell::new(None),
        }
    }
    pub(crate) fn set_ready_peers(&self, peers: bool) {
        self.peers.set(peers);
    }
    pub(crate) fn ready_peer_at(&self, at: Instant) {
        self.peer_at.set(Some(at));
    }
}
impl<const N: usize> Deref for VirtualClock<N> {
    type Target = oer_time_virtual::VirtualClock<N>;
    fn deref(&self) -> &Self::Target {
        &self.clock
    }
}
impl<const N: usize> Clock for VirtualClock<N> {
    fn now(&self) -> Instant {
        self.clock.now()
    }
}
impl<const N: usize> Timer for VirtualClock<N> {
    fn wait_until(&self, at: Instant) -> impl core::future::Future<Output = ()> {
        self.clock.wait_until(at)
    }
}
impl<const N: usize> oer_cooperate::CooperativeTimer for VirtualClock<N> {
    fn should_yield(&self) -> bool {
        self.peers.get() || self.peer_at.get().is_some_and(|at| self.clock.now() >= at)
    }
}

impl<const N: usize> Default for VirtualClock<N> {
    fn default() -> Self {
        Self::new()
    }
}
