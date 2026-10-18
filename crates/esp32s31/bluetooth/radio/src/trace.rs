//! The role's trace points (`oer-bluetooth-trace`), recorded behind the
//! `trace` feature.

use oer_bluetooth_radio::{LeCapabilities, RadioRequest, RequestError};

use crate::RadioFaulted;
use oer_bluetooth_trace::{RequestAnswer, RequestKind};
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
/// tests observe only their own radio.
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

/// The events of type `E` this thread emitted since they were last taken;
/// the other events stay.
#[cfg(test)]
pub(crate) fn take<E: Event>() -> std::vec::Vec<E> {
    RECORDED.with(|recorded| {
        let mut recorded = recorded.borrow_mut();
        let (taken, kept) = core::mem::take(&mut *recorded)
            .into_iter()
            .partition::<std::vec::Vec<_>, _>(|record| record.kind == E::KIND.raw());
        *recorded = kept;
        taken
            .into_iter()
            .map(|record| E::decode(record.words).expect("a recorded event decodes"))
            .collect()
    })
}

/// The kind of `request`.
pub(crate) const fn request_kind<K: LeCapabilities>(request: &RadioRequest<'_, K>) -> RequestKind {
    match request {
        RadioRequest::ConfigureAdvertising(_) => RequestKind::ConfigureAdvertising,
        RadioRequest::Advertise(_) => RequestKind::Advertise,
        RadioRequest::RemoveAdvertising(_) => RequestKind::RemoveAdvertising,
        RadioRequest::ConfigureScanner(_) => RequestKind::ConfigureScanner,
        RadioRequest::Scan(_) => RequestKind::Scan,
        RadioRequest::RemoveScanner(_) => RequestKind::RemoveScanner,
        RadioRequest::OpenConnection(_) => RequestKind::OpenConnection,
        RadioRequest::ConnectionEvent(_) => RequestKind::ConnectionEvent,
        RadioRequest::Transmit { .. } => RequestKind::Transmit,
        RadioRequest::CloseConnection(_) => RequestKind::CloseConnection,
        RadioRequest::TestTransmit(_) => RequestKind::TestTransmit,
        RadioRequest::TestReceive(_) => RequestKind::TestReceive,
        RadioRequest::EndTest => RequestKind::EndTest,
        RadioRequest::FilterAcceptList(_) => RequestKind::FilterAcceptList,
    }
}

/// The answer a request's result stands for.
pub(crate) const fn answer(
    result: Result<Result<(), RequestError>, RadioFaulted>,
) -> RequestAnswer {
    match result {
        Ok(Ok(())) => RequestAnswer::Accepted,
        Ok(Err(RequestError::ClockUnavailable)) => RequestAnswer::ClockUnavailable,
        Ok(Err(RequestError::EpochExhausted)) => RequestAnswer::EpochExhausted,
        Ok(Err(RequestError::NoInstance)) => RequestAnswer::NoInstance,
        Ok(Err(RequestError::Unknown)) => RequestAnswer::Unknown,
        Ok(Err(RequestError::AlreadyConfigured)) => RequestAnswer::AlreadyConfigured,
        Ok(Err(RequestError::Busy)) => RequestAnswer::Busy,
        Ok(Err(RequestError::Late)) => RequestAnswer::Late,
        Ok(Err(RequestError::ListFull)) => RequestAnswer::ListFull,
        Ok(Err(RequestError::NotListed)) => RequestAnswer::NotListed,
        Err(RadioFaulted) => RequestAnswer::Faulted,
    }
}
