//! The runtime's trace points (`oer-i-trace`), recorded behind the
//! `trace` feature.

use oer_trace::Event;

/// Record the event `make` builds, when its channel is enabled. Without the
/// `trace` feature nothing is built.
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn emit<E: Event>(make: impl FnOnce() -> E) {
    #[cfg(feature = "trace")]
    if oer_trace::enabled(E::CHANNEL) {
        oer_trace::emit(&make());
    }
    #[cfg(not(feature = "trace"))]
    let _ = make;
}

/// Host tests record every event on the calling thread instead, so parallel
/// tests observe only their own runtime.
#[cfg(test)]
pub(crate) fn emit<E: Event>(make: impl FnOnce() -> E) {
    let record = oer_trace::Record {
        tag: 1,
        kind: E::KIND.raw(),
        t_us: 0,
        words: make().encode(),
    };
    RECORDED.with(|recorded| recorded.borrow_mut().push(record));
}

#[cfg(test)]
std::thread_local! {
    static RECORDED: core::cell::RefCell<std::vec::Vec<oer_trace::Record>> =
        const { core::cell::RefCell::new(std::vec::Vec::new()) };
}

/// The records this thread emitted since the last call.
#[cfg(test)]
pub(crate) fn take_recorded() -> std::vec::Vec<oer_trace::Record> {
    RECORDED.with(|recorded| core::mem::take(&mut *recorded.borrow_mut()))
}
