//! Requests for one cross-hart sample, shared by every requester.
//!
//! One hart samples another by raising a software interrupt line whose
//! handler, bound on the sampled hart, records the context it interrupted.
//! Several requesters share that one line: each sets its own flag in
//! [`SampleRequests`], then raises the line. The handler clears the line,
//! then [takes](SampleRequests::take) every pending flag and serves each
//! requester it names, so one raise serves every request made before it.
//!
//! A request set after the handler cleared the line raises it again: that
//! later entry finds the flag, or nothing if the earlier entry already took
//! it. A handler that clears the line after taking the flags could drop a
//! request set between the two.

use core::sync::atomic::{AtomicU8, Ordering};

/// A requester of the cross-hart sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requester {
    /// The hang watchdog, for the sampled hart's interrupted context.
    HangWatchdog = 0,
    /// The program-counter profiler, for one profile sample.
    Profiler = 1,
}

impl Requester {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The pending request flags, one per [`Requester`].
pub struct SampleRequests(AtomicU8);

impl SampleRequests {
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    /// Set `requester`'s flag; the caller raises the line after it.
    pub fn request(&self, requester: Requester) {
        self.0.fetch_or(requester.bit(), Ordering::Release);
    }

    /// Take and clear every pending request.
    pub fn take(&self) -> Pending {
        Pending(self.0.swap(0, Ordering::Acquire))
    }
}

impl Default for SampleRequests {
    fn default() -> Self {
        Self::new()
    }
}

/// The requests one handler entry serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending(u8);

impl Pending {
    /// Whether `requester` asked for this sample.
    pub const fn contains(self, requester: Requester) -> bool {
        self.0 & requester.bit() != 0
    }

    /// Whether no requester asked.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_take_serves_every_pending_requester() {
        let requests = SampleRequests::new();
        requests.request(Requester::HangWatchdog);
        requests.request(Requester::Profiler);
        let pending = requests.take();
        assert!(pending.contains(Requester::HangWatchdog));
        assert!(pending.contains(Requester::Profiler));
    }

    #[test]
    fn a_served_request_is_cleared() {
        let requests = SampleRequests::new();
        requests.request(Requester::Profiler);
        let pending = requests.take();
        assert!(pending.contains(Requester::Profiler));
        assert!(!pending.contains(Requester::HangWatchdog));
        assert!(requests.take().is_empty());
    }

    #[test]
    fn a_repeated_request_is_served_once() {
        let requests = SampleRequests::new();
        requests.request(Requester::HangWatchdog);
        requests.request(Requester::HangWatchdog);
        assert!(requests.take().contains(Requester::HangWatchdog));
        assert!(requests.take().is_empty());
    }
}
