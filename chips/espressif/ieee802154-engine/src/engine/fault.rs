//! Impossible hardware inputs returned as values, retaining source observations.

use super::Ieee802154State;
use oer_espressif_ieee802154_mac_values::*;

/// Impossible service input, with source observations and the completed service prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// A sample includes an unnamed lane or the unsupported clock-match event.
    UnhandledEvents {
        /// Complete sampled event set, including each unnamed lane separately.
        observation: Ieee802154EventObservation,
        /// Software state at entry, before any event was handled.
        state: Ieee802154State,
    },
    /// A supported sample failed an invariant during ordered dispatch.
    Service {
        /// Typed failure at the current stage.
        reason: ServiceError,
        /// Original complete event sample.
        observation: Ieee802154EventObservation,
        /// Software state before service.
        entry_state: Ieee802154State,
        /// Actual state at failure; completed work is never rolled back.
        state: Ieee802154State,
        /// Sampled receive-abort classification.
        rx_abort: Ieee802154RxAbortReasonObservation,
        /// Sampled transmit-abort classification.
        tx_abort: Ieee802154TxAbortReasonObservation,
        /// Exact completed prefix and failing stage.
        progress: ServiceProgress,
    },
}

/// Invariant failures returned by ordered service helpers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceError {
    /// The event is incompatible with the current software state.
    EventState(Ieee802154Event),
    /// The RX-abort reason has no supported identity; timers are already stopped.
    RxAbortUnclassified(Ieee802154RxStatus),
    /// RX-abort classification disagrees with state; timers are already stopped.
    RxAbortState(Ieee802154RxStatus),
    /// The TX-abort reason has no supported identity; security and timers are cleared.
    TxAbortUnclassified,
    /// Receive metadata observed an unsupported channel code.
    FrequencyCode(u8),
    /// PHR metadata does not fit the fixed DMA image.
    FrameMetadata(u8),
    /// A diagnostic total cannot represent the next count.
    Statistics(super::statistics::StatisticsOverflow),
}

/// The current dispatch stage. Its documented prefix has already happened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceStage {
    /// Diagnostic reads/drains precede event acknowledgement.
    Statistics,
    /// Between complete event handlers; the current handler has no committed prefix.
    Dispatch,
    /// First RX-abort pass: timers stopped and RX status sampled.
    RxAbort,
    /// RX_DONE: timers stopped before reading frame metadata.
    RxDone,
    /// ACK_RX_DONE: timer0 stopped, acknowledged and disabled before metadata.
    AckRxDone,
    /// TX-abort pass: security cleared and timers stopped.
    TxAbort,
}

/// Progress within the original sample; no bit authorizes an acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceProgress {
    /// Whether the original supported event sample has been acknowledged.
    pub acknowledged: bool,
    /// Events whose dispatch returned successfully, in vendor order.
    pub completed: Ieee802154EventMask,
    /// Whether the first (pre-SFD) RX-abort pass completed.
    pub rx_abort_phase_done: bool,
    /// Whether either abort pass already notified the environment of coexistence break.
    pub coexistence_break_notified: bool,
    /// Whether the diagnostic record/drain pass completed before dispatch.
    pub statistics_completed: bool,
    /// Current stage, including its completed helper prefix.
    pub stage: ServiceStage,
}
