//! The engine's trace points (`oer-i-trace`), recorded behind the
//! `trace` feature. Every conversion is integer-only, so the interrupt
//! handler emits with the FPU off.

use oer_ieee802154_trace::{
    AbortReason, InterruptEvents, MacState, RxAbortReason, TimerCallback, TimerId, TxAbortReason,
    TxFailure,
};
use oer_trace::Event;

use super::{Ieee802154State, Ieee802154TxError, TimerAction};
use crate::ll::Ieee802154Timer;
use crate::types::{
    Ieee802154Event, Ieee802154EventObservation, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReason,
    Ieee802154TxAbortReasonObservation,
};

/// Record the event `make` builds, when its channel is enabled. Without the
/// `trace` feature nothing is built or read.
#[cfg(not(test))]
#[inline(always)]
pub(super) fn emit<E: Event>(make: impl FnOnce() -> E) {
    #[cfg(feature = "trace")]
    if oer_trace::enabled(E::CHANNEL) {
        oer_trace::emit(&make());
    }
    #[cfg(not(feature = "trace"))]
    let _ = make;
}

/// Host tests record every event on the calling thread instead, so parallel
/// tests observe only their own engine.
#[cfg(test)]
pub(super) fn emit<E: Event>(make: impl FnOnce() -> E) {
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
pub(super) fn take_recorded() -> std::vec::Vec<oer_trace::Record> {
    RECORDED.with(|recorded| core::mem::take(&mut *recorded.borrow_mut()))
}

pub(super) const fn state(state: Ieee802154State) -> MacState {
    match state {
        Ieee802154State::Disable => MacState::Disable,
        Ieee802154State::Idle => MacState::Idle,
        Ieee802154State::Sleep => MacState::Sleep,
        Ieee802154State::Rx => MacState::Rx,
        Ieee802154State::TxAck => MacState::TxAck,
        Ieee802154State::TxEnhAck => MacState::TxEnhAck,
        Ieee802154State::TxCca => MacState::TxCca,
        Ieee802154State::Tx => MacState::Tx,
        Ieee802154State::RxAck => MacState::RxAck,
        Ieee802154State::Ed => MacState::Ed,
        Ieee802154State::Cca => MacState::Cca,
    }
}

pub(super) const fn tx_failure(error: Ieee802154TxError) -> TxFailure {
    match error {
        Ieee802154TxError::CcaBusy => TxFailure::CcaBusy,
        Ieee802154TxError::Abort => TxFailure::Abort,
        Ieee802154TxError::NoAck => TxFailure::NoAck,
        Ieee802154TxError::InvalidAck => TxFailure::InvalidAck,
        Ieee802154TxError::Coexist => TxFailure::Coexist,
        Ieee802154TxError::Security => TxFailure::Security,
    }
}

pub(super) const fn rx_abort(reason: Ieee802154RxAbortReasonObservation) -> AbortReason {
    use Ieee802154RxAbortReason as R;
    let Ieee802154RxAbortReasonObservation::Named(reason) = reason else {
        return AbortReason::Rx(None);
    };
    AbortReason::Rx(Some(match reason {
        R::RxStop => RxAbortReason::RxStop,
        R::SfdTimeout => RxAbortReason::SfdTimeout,
        R::CrcError => RxAbortReason::CrcError,
        R::InvalidLength => RxAbortReason::InvalidLength,
        R::FilterFail => RxAbortReason::FilterFail,
        R::NoRss => RxAbortReason::NoRss,
        R::CoexistenceBreak => RxAbortReason::CoexistenceBreak,
        R::UnexpectedAck => RxAbortReason::UnexpectedAck,
        R::RxRestart => RxAbortReason::RxRestart,
        R::TxAckTimeout => RxAbortReason::TxAckTimeout,
        R::TxAckStop => RxAbortReason::TxAckStop,
        R::TxAckCoexistenceBreak => RxAbortReason::TxAckCoexistenceBreak,
        R::EnhancedAckSecurityError => RxAbortReason::EnhancedAckSecurityError,
        R::EdAbort => RxAbortReason::EdAbort,
        R::EdStop => RxAbortReason::EdStop,
        R::EdCoexistenceReject => RxAbortReason::EdCoexistenceReject,
    }))
}

pub(super) const fn tx_abort(reason: Ieee802154TxAbortReasonObservation) -> AbortReason {
    use Ieee802154TxAbortReason as R;
    let Ieee802154TxAbortReasonObservation::Named(reason) = reason else {
        return AbortReason::Tx(None);
    };
    AbortReason::Tx(Some(match reason {
        R::RxAckStop => TxAbortReason::RxAckStop,
        R::RxAckSfdTimeout => TxAbortReason::RxAckSfdTimeout,
        R::RxAckCrcError => TxAbortReason::RxAckCrcError,
        R::RxAckInvalidLength => TxAbortReason::RxAckInvalidLength,
        R::RxAckFilterFail => TxAbortReason::RxAckFilterFail,
        R::RxAckNoRss => TxAbortReason::RxAckNoRss,
        R::RxAckCoexistenceBreak => TxAbortReason::RxAckCoexistenceBreak,
        R::RxAckTypeNotAck => TxAbortReason::RxAckTypeNotAck,
        R::RxAckRestart => TxAbortReason::RxAckRestart,
        R::RxAckTimeout => TxAbortReason::RxAckTimeout,
        R::TxStop => TxAbortReason::TxStop,
        R::TxCoexistenceBreak => TxAbortReason::TxCoexistenceBreak,
        R::TxSecurityError => TxAbortReason::TxSecurityError,
        R::CcaFailed => TxAbortReason::CcaFailed,
        R::CcaBusy => TxAbortReason::CcaBusy,
    }))
}

pub(super) const fn events(observation: Ieee802154EventObservation) -> InterruptEvents {
    use Ieee802154Event as E;
    use InterruptEvents as I;
    I::NONE
        .with_if(I::TX_DONE, observation.contains(E::TxDone))
        .with_if(I::RX_DONE, observation.contains(E::RxDone))
        .with_if(I::ACK_TX_DONE, observation.contains(E::AckTxDone))
        .with_if(I::ACK_RX_DONE, observation.contains(E::AckRxDone))
        .with_if(I::RX_ABORT, observation.contains(E::RxAbort))
        .with_if(I::TX_ABORT, observation.contains(E::TxAbort))
        .with_if(I::ED_DONE, observation.contains(E::EdDone))
        .with_if(I::TIMER0_OVERFLOW, observation.contains(E::Timer0Overflow))
        .with_if(I::TIMER1_OVERFLOW, observation.contains(E::Timer1Overflow))
        .with_if(
            I::CLOCK_COUNT_MATCH,
            observation.contains(E::ClockCountMatch),
        )
        .with_if(I::TX_SFD_DONE, observation.contains(E::TxSfdDone))
        .with_if(I::RX_SFD_DONE, observation.contains(E::RxSfdDone))
        .with_if(I::UNCLASSIFIED, observation.classification().is_err())
}

pub(super) const fn timer(timer: Ieee802154Timer) -> TimerId {
    match timer {
        Ieee802154Timer::Timer0 => TimerId::Timer0,
        Ieee802154Timer::Timer1 => TimerId::Timer1,
    }
}

pub(super) const fn callback(action: Option<TimerAction>) -> TimerCallback {
    match action {
        None => TimerCallback::None,
        Some(TimerAction::AckTimeout) => TimerCallback::AckTimeout,
        Some(TimerAction::StartReceiveAt { .. }) => TimerCallback::StartReceiveAt,
        Some(TimerAction::FinishReceiveAt) => TimerCallback::FinishReceiveAt,
    }
}
