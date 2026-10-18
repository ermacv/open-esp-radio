//! Reset-isolated IEEE 802.15.4 validation probes, the single-device air
//! check, host-driven peer sessions and HIL evidence mapping.

#[cfg(feature = "i-radio")]
mod air_check;
#[cfg(feature = "i-radio")]
mod client;
#[cfg(feature = "i-radio")]
mod session;
#[cfg(feature = "i-radio")]
pub(super) use air_check::run_air_check;
#[cfg(feature = "i-radio")]
pub(super) use session::run_session;
#[cfg(feature = "i-thread")]
mod thread;
#[cfg(feature = "i-thread")]
pub(super) use thread::run_thread;
#[cfg(feature = "i-route-probe")]
mod route_probe;
#[cfg(feature = "i-route-probe")]
use oer_esp32s31_hal::{
    ieee802154::{Ieee802154Cold, Ieee802154FoundationConfigured},
    root::RadioHardware,
};
#[cfg(feature = "i-route-probe")]
use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
#[cfg(feature = "i-route-probe")]
pub(crate) use route_probe::route_probe_interrupt;
#[cfg(feature = "i-route-probe")]
pub(super) use route_probe::run_route_probe;

#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
use oer_esp32s31_radio_esp_hal::EspHalRadioClocks;
#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;

#[cfg(feature = "i-event-status-probe")]
use oer_esp32s31_hal::ieee802154::{
    Ieee802154EventStatusProbeConfig,
    Ieee802154EventStatusProbeEvidence as HalIeee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeIsolation,
    Ieee802154EventStatusProbeStop as HalIeee802154EventStatusProbeStop,
};
#[cfg(feature = "i-ed-event-probe")]
use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154EdEventProbeConfig,
        Ieee802154EdEventProbeEvidence as HalIeee802154EdEventProbeEvidence,
        Ieee802154EdEventProbeIsolation,
        Ieee802154EdEventProbeStop as HalIeee802154EdEventProbeStop, Ieee802154MacControl,
        Ieee802154MacPolicy, Ieee802154OperationEventMaskState, Ieee802154OperationPollBudget,
        Ieee802154OperationRxAbortMaskState, Ieee802154OperationStage, Ieee802154PanIdentity,
        Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
        Ieee802154PolledOperationResult,
    },
    types::{
        Ieee802154OperationRxAbortEnableObservation,
        Ieee802154RxAbortReason as HalIeee802154RxAbortReason,
        Ieee802154RxAbortReasonObservation as HalIeee802154RxAbortReasonObservation,
        Ieee802154ValidationEdDurationState as HalIeee802154ValidationEdDurationState,
    },
};
#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
use oer_esp32s31_hal::{
    ieee802154::{Ieee802154Cold, Ieee802154FoundationConfigured},
    root::RadioHardware,
    types::{
        Ieee802154ObservedEventState as HalIeee802154ObservedEventState,
        Ieee802154ValidationEventEnableState as HalIeee802154ValidationEventEnableState,
    },
};

#[cfg(feature = "i-ed-event-probe")]
use oer_hil_protocol::{
    ieee802154::Ieee802154EdEventProbeEvidence, ieee802154::Ieee802154EdEventProbeRequest,
    ieee802154::Ieee802154EdEventProbeStop, ieee802154::Ieee802154PolledEdMaskState,
    ieee802154::Ieee802154PolledEdOutcome, ieee802154::Ieee802154PolledEdStage,
    ieee802154::Ieee802154RxAbortObservation, ieee802154::Ieee802154RxAbortReason,
    ieee802154::Ieee802154ValidationEdDurationState,
    ieee802154::Ieee802154ValidationRxAbortEnableState,
};
#[cfg(feature = "i-event-status-probe")]
use oer_hil_protocol::{
    ieee802154::Ieee802154EventStatusProbeEvidence, ieee802154::Ieee802154EventStatusProbeRequest,
    ieee802154::Ieee802154EventStatusProbeStop,
};
#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
use oer_hil_protocol::{
    ieee802154::Ieee802154ObservedEventState, ieee802154::Ieee802154ValidationEventEnableState,
};

/// Bring the IEEE 802.15.4 client of a concurrent split to its MAC
/// foundation: common power, module clocks, MAC reset, foundation.
///
/// The radio peripheral token proves the image owns the radio; the modem
/// clocks' platform sources come from ESP-HAL's counted clock tree. The probe
/// image is reset-isolated and terminal: the module clocks and common power
/// stay on, and the shared radio that holds their platform references lives
/// in a static for the rest of the boot, so the MAC never runs with a clock
/// its bookkeeping dropped. The other partitions are dropped without
/// release.
#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe",
    feature = "ieee802154-route-probe"
))]
fn ieee802154_foundation(mut radio: EspHalRadioPlatform) -> Option<Ieee802154FoundationConfigured> {
    static SHARED: static_cell::StaticCell<oer_esp32s31_hal::shared_radio::SharedRadio> =
        static_cell::StaticCell::new();
    let (shared, partitions) =
        RadioHardware::take(radio.analog_bus_ownership()?)?.into_concurrent(());
    let shared: &'static _ = SHARED.try_init(shared)?;
    let mut lease = shared.try_acquire().ok()?;
    // The terminal probe never leaves common power, so its proof ends with
    // the boot.
    let (powered, _power) = Ieee802154Cold::from_partition(partitions.ieee802154)
        .power_up(&mut lease, &mut EspHalRadioClocks::new())
        .ok()?;
    let clocked = powered
        .enable_clocks(&mut lease, &mut EspHalRadioClocks::new())
        .ok()?;
    let reset = clocked.reset_mac(&mut lease).ok()?;
    reset.configure_foundation().ok()
}

#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
const fn map_ieee802154_observed_events(
    state: HalIeee802154ObservedEventState,
) -> Ieee802154ObservedEventState {
    match state {
        HalIeee802154ObservedEventState::Clear => Ieee802154ObservedEventState::Clear,
        HalIeee802154ObservedEventState::Timer0Only => Ieee802154ObservedEventState::Timer0Only,
        HalIeee802154ObservedEventState::Timer1Only => Ieee802154ObservedEventState::Timer1Only,
        HalIeee802154ObservedEventState::Timer0AndTimer1 => {
            Ieee802154ObservedEventState::Timer0AndTimer1
        }
        HalIeee802154ObservedEventState::EdDoneOnly => Ieee802154ObservedEventState::EdDoneOnly,
        HalIeee802154ObservedEventState::EdDoneAndTimer0 => {
            Ieee802154ObservedEventState::EdDoneAndTimer0
        }
        HalIeee802154ObservedEventState::RxAbortOnly => Ieee802154ObservedEventState::RxAbortOnly,
        HalIeee802154ObservedEventState::RxAbortWithOther => {
            Ieee802154ObservedEventState::RxAbortWithOther
        }
        HalIeee802154ObservedEventState::EdDoneWithOther => {
            Ieee802154ObservedEventState::EdDoneWithOther
        }
        HalIeee802154ObservedEventState::EdDoneAndRxAbortWithOther => {
            Ieee802154ObservedEventState::EdDoneAndRxAbortWithOther
        }
        HalIeee802154ObservedEventState::UnexpectedNamed => {
            Ieee802154ObservedEventState::UnexpectedNamed
        }
        HalIeee802154ObservedEventState::Unclassified => Ieee802154ObservedEventState::Unclassified,
    }
}

#[cfg(any(
    feature = "ieee802154-event-status-probe",
    feature = "ieee802154-ed-event-probe"
))]
const fn map_ieee802154_validation_event_enable(
    state: HalIeee802154ValidationEventEnableState,
) -> Ieee802154ValidationEventEnableState {
    match state {
        HalIeee802154ValidationEventEnableState::AllMasked => {
            Ieee802154ValidationEventEnableState::AllMasked
        }
        HalIeee802154ValidationEventEnableState::TimerPairOnly => {
            Ieee802154ValidationEventEnableState::TimerPairOnly
        }
        HalIeee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly => {
            Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly
        }
        HalIeee802154ValidationEventEnableState::Unexpected => {
            Ieee802154ValidationEventEnableState::Unexpected
        }
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_validation_ed_duration(
    state: HalIeee802154ValidationEdDurationState,
) -> Ieee802154ValidationEdDurationState {
    match state {
        HalIeee802154ValidationEdDurationState::ValidationEight => {
            Ieee802154ValidationEdDurationState::ValidationEight
        }
        HalIeee802154ValidationEdDurationState::Other => Ieee802154ValidationEdDurationState::Other,
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_validation_rx_abort_enable(
    state: Ieee802154OperationRxAbortEnableObservation,
) -> Ieee802154ValidationRxAbortEnableState {
    match state {
        Ieee802154OperationRxAbortEnableObservation::AllMasked => {
            Ieee802154ValidationRxAbortEnableState::AllMasked
        }
        Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly => {
            Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly
        }
        Ieee802154OperationRxAbortEnableObservation::Unexpected => {
            Ieee802154ValidationRxAbortEnableState::Unexpected
        }
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_rx_abort_reason(
    reason: HalIeee802154RxAbortReason,
) -> Ieee802154RxAbortReason {
    match reason {
        HalIeee802154RxAbortReason::RxStop => Ieee802154RxAbortReason::RxStop,
        HalIeee802154RxAbortReason::SfdTimeout => Ieee802154RxAbortReason::SfdTimeout,
        HalIeee802154RxAbortReason::CrcError => Ieee802154RxAbortReason::CrcError,
        HalIeee802154RxAbortReason::InvalidLength => Ieee802154RxAbortReason::InvalidLength,
        HalIeee802154RxAbortReason::FilterFail => Ieee802154RxAbortReason::FilterFail,
        HalIeee802154RxAbortReason::NoRss => Ieee802154RxAbortReason::NoRss,
        HalIeee802154RxAbortReason::CoexistenceBreak => Ieee802154RxAbortReason::CoexistenceBreak,
        HalIeee802154RxAbortReason::UnexpectedAck => Ieee802154RxAbortReason::UnexpectedAck,
        HalIeee802154RxAbortReason::RxRestart => Ieee802154RxAbortReason::RxRestart,
        HalIeee802154RxAbortReason::TxAckTimeout => Ieee802154RxAbortReason::TxAckTimeout,
        HalIeee802154RxAbortReason::TxAckStop => Ieee802154RxAbortReason::TxAckStop,
        HalIeee802154RxAbortReason::TxAckCoexistenceBreak => {
            Ieee802154RxAbortReason::TxAckCoexistenceBreak
        }
        HalIeee802154RxAbortReason::EnhancedAckSecurityError => {
            Ieee802154RxAbortReason::EnhancedAckSecurityError
        }
        HalIeee802154RxAbortReason::EdAbort => Ieee802154RxAbortReason::EdAbort,
        HalIeee802154RxAbortReason::EdStop => Ieee802154RxAbortReason::EdStop,
        HalIeee802154RxAbortReason::EdCoexistenceReject => {
            Ieee802154RxAbortReason::EdCoexistenceReject
        }
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_rx_abort_observation(
    observation: HalIeee802154RxAbortReasonObservation,
) -> Ieee802154RxAbortObservation {
    match observation {
        HalIeee802154RxAbortReasonObservation::Named(reason) => {
            Ieee802154RxAbortObservation::Named(map_ieee802154_rx_abort_reason(reason))
        }
        HalIeee802154RxAbortReasonObservation::Unclassified => {
            Ieee802154RxAbortObservation::Unclassified
        }
    }
}

#[cfg(feature = "i-event-status-probe")]
const fn unsupported_ieee802154_event_status_probe() -> Ieee802154EventStatusProbeEvidence {
    Ieee802154EventStatusProbeEvidence {
        stop: Ieee802154EventStatusProbeStop::UnsupportedSetup,
        event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_active: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
        post_enable_events: Ieee802154ObservedEventState::Clear,
        timer0_value_before_start: 0,
        timer1_value_before_start: 0,
        timer0_value_min: 0,
        timer0_value_max: 0,
        timer1_value_min: 0,
        timer1_value_max: 0,
        timer0_value_after_stop: 0,
        timer1_value_after_stop: 0,
        reset_events: Ieee802154ObservedEventState::Clear,
        dual_observed_events: Ieee802154ObservedEventState::Clear,
        dual_latched_events: Ieee802154ObservedEventState::Clear,
        after_timer0_ack_events: Ieee802154ObservedEventState::Clear,
        after_timer1_ack_events: Ieee802154ObservedEventState::Clear,
        distinct_snapshot_events: Ieee802154ObservedEventState::Clear,
        distinct_before_ack_events: Ieee802154ObservedEventState::Clear,
        distinct_after_ack_events: Ieee802154ObservedEventState::Clear,
        cleanup_pending_events: Ieee802154ObservedEventState::Clear,
        final_events: Ieee802154ObservedEventState::Clear,
    }
}

#[cfg(feature = "i-event-status-probe")]
const fn map_ieee802154_event_status_probe(
    evidence: HalIeee802154EventStatusProbeEvidence,
) -> Ieee802154EventStatusProbeEvidence {
    let stop = match evidence.stop {
        HalIeee802154EventStatusProbeStop::Complete => Ieee802154EventStatusProbeStop::Complete,
        HalIeee802154EventStatusProbeStop::UnsupportedSetup => {
            Ieee802154EventStatusProbeStop::UnsupportedSetup
        }
        HalIeee802154EventStatusProbeStop::RouteNotQuiesced => {
            Ieee802154EventStatusProbeStop::RouteNotQuiesced
        }
        HalIeee802154EventStatusProbeStop::ResetNotClear => {
            Ieee802154EventStatusProbeStop::ResetNotClear
        }
        HalIeee802154EventStatusProbeStop::EventEnableReadbackMismatch => {
            Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch
        }
        HalIeee802154EventStatusProbeStop::PostEnableStatusNotClear => {
            Ieee802154EventStatusProbeStop::PostEnableStatusNotClear
        }
        HalIeee802154EventStatusProbeStop::TimerActivityTimeout => {
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        }
        HalIeee802154EventStatusProbeStop::DualLatchTimeout => {
            Ieee802154EventStatusProbeStop::DualLatchTimeout
        }
        HalIeee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch => {
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
        }
        HalIeee802154EventStatusProbeStop::DistinctFirstLatchTimeout => {
            Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout
        }
        HalIeee802154EventStatusProbeStop::DistinctSecondLatchTimeout => {
            Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout
        }
        HalIeee802154EventStatusProbeStop::CleanupNotClear => {
            Ieee802154EventStatusProbeStop::CleanupNotClear
        }
    };
    Ieee802154EventStatusProbeEvidence {
        stop,
        event_enable_before: map_ieee802154_validation_event_enable(evidence.event_enable_before),
        event_enable_active: map_ieee802154_validation_event_enable(evidence.event_enable_active),
        event_enable_after: map_ieee802154_validation_event_enable(evidence.event_enable_after),
        post_enable_events: map_ieee802154_observed_events(evidence.post_enable_events),
        timer0_value_before_start: evidence.timer0_value_before_start,
        timer1_value_before_start: evidence.timer1_value_before_start,
        timer0_value_min: evidence.timer0_value_min,
        timer0_value_max: evidence.timer0_value_max,
        timer1_value_min: evidence.timer1_value_min,
        timer1_value_max: evidence.timer1_value_max,
        timer0_value_after_stop: evidence.timer0_value_after_stop,
        timer1_value_after_stop: evidence.timer1_value_after_stop,
        reset_events: map_ieee802154_observed_events(evidence.reset_events),
        dual_observed_events: map_ieee802154_observed_events(evidence.dual_observed_events),
        dual_latched_events: map_ieee802154_observed_events(evidence.dual_latched_events),
        after_timer0_ack_events: map_ieee802154_observed_events(evidence.after_timer0_ack_events),
        after_timer1_ack_events: map_ieee802154_observed_events(evidence.after_timer1_ack_events),
        distinct_snapshot_events: map_ieee802154_observed_events(evidence.distinct_snapshot_events),
        distinct_before_ack_events: map_ieee802154_observed_events(
            evidence.distinct_before_ack_events,
        ),
        distinct_after_ack_events: map_ieee802154_observed_events(
            evidence.distinct_after_ack_events,
        ),
        cleanup_pending_events: map_ieee802154_observed_events(evidence.cleanup_pending_events),
        final_events: map_ieee802154_observed_events(evidence.final_events),
    }
}

/// Run the bounded timer-bit discriminator without routing an interrupt or
/// exposing the validation-only acknowledge capability to production code.
#[cfg(feature = "i-event-status-probe")]
pub(super) fn run_event_status_probe(
    platform: EspHalRadioPlatform,
    request: Ieee802154EventStatusProbeRequest,
) -> Ieee802154EventStatusProbeEvidence {
    let Some(config) =
        Ieee802154EventStatusProbeConfig::new(request.poll_limit, request.timer_threshold)
    else {
        return unsupported_ieee802154_event_status_probe();
    };
    let Some(isolation) = Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image()
    else {
        return unsupported_ieee802154_event_status_probe();
    };
    let Some(foundation) = ieee802154_foundation(platform) else {
        return unsupported_ieee802154_event_status_probe();
    };
    let finished = foundation.validation_probe_event_status(config, isolation);
    map_ieee802154_event_status_probe(*finished.evidence())
}

#[cfg(feature = "i-ed-event-probe")]
const fn unsupported_ieee802154_ed_event_probe() -> Ieee802154EdEventProbeEvidence {
    Ieee802154EdEventProbeEvidence {
        stop: Ieee802154EdEventProbeStop::UnsupportedSetup,
        production_ed_first: Ieee802154PolledEdOutcome::NotRun,
        production_ed_second: None,
        event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_active: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
        rx_abort_enable_before: Ieee802154ValidationRxAbortEnableState::AllMasked,
        rx_abort_enable_active: Ieee802154ValidationRxAbortEnableState::AllMasked,
        rx_abort_enable_after: Ieee802154ValidationRxAbortEnableState::AllMasked,
        ed_duration_before: Ieee802154ValidationEdDurationState::Other,
        ed_duration_active: Ieee802154ValidationEdDurationState::Other,
        ed_duration_after: Ieee802154ValidationEdDurationState::Other,
        timer0_value_before_start: 0,
        timer0_value_min: 0,
        timer0_value_max: 0,
        timer0_value_after_stop: 0,
        reset_events: Ieee802154ObservedEventState::Clear,
        post_enable_events: Ieee802154ObservedEventState::Clear,
        observed_events: Ieee802154ObservedEventState::Clear,
        terminal_events: Ieee802154ObservedEventState::Clear,
        after_ed_done_write_events: Ieee802154ObservedEventState::Clear,
        after_timer0_write_events: Ieee802154ObservedEventState::Clear,
        cleanup_pending_events: Ieee802154ObservedEventState::Clear,
        final_events: Ieee802154ObservedEventState::Clear,
        rx_abort_reason: None,
        stop_command_issued: false,
        cleanup_clear: false,
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_polled_stage(stage: Ieee802154OperationStage) -> Ieee802154PolledEdStage {
    match stage {
        Ieee802154OperationStage::Prepare => Ieee802154PolledEdStage::Prepare,
        Ieee802154OperationStage::StartEventWindow => Ieee802154PolledEdStage::StartEventWindow,
        Ieee802154OperationStage::StartCommand => Ieee802154PolledEdStage::StartCommand,
        Ieee802154OperationStage::Poll => Ieee802154PolledEdStage::Poll,
        Ieee802154OperationStage::TerminalSample => Ieee802154PolledEdStage::TerminalSample,
        Ieee802154OperationStage::AcknowledgeTerminalEvent => {
            Ieee802154PolledEdStage::AcknowledgeTerminalEvent
        }
        Ieee802154OperationStage::Cleanup => Ieee802154PolledEdStage::Cleanup,
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_event_mask(
    state: Ieee802154OperationEventMaskState,
) -> Ieee802154PolledEdMaskState {
    match state {
        Ieee802154OperationEventMaskState::AllMasked => Ieee802154PolledEdMaskState::AllMasked,
        Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly => {
            Ieee802154PolledEdMaskState::OperationOnly
        }
        Ieee802154OperationEventMaskState::Unexpected => Ieee802154PolledEdMaskState::Unexpected,
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_rx_abort_mask(
    state: Ieee802154OperationRxAbortMaskState,
) -> Ieee802154PolledEdMaskState {
    match state {
        Ieee802154OperationRxAbortMaskState::AllMasked => Ieee802154PolledEdMaskState::AllMasked,
        Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly => {
            Ieee802154PolledEdMaskState::OperationOnly
        }
        Ieee802154OperationRxAbortMaskState::Unexpected => Ieee802154PolledEdMaskState::Unexpected,
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_polled_failure(
    failure: Ieee802154PolledOperationFailure,
) -> Ieee802154PolledEdOutcome {
    match failure {
        Ieee802154PolledOperationFailure::Aborted(evidence) => Ieee802154PolledEdOutcome::Aborted {
            event_status: map_ieee802154_observed_events(evidence.event_status().state()),
            rx_abort_reason: map_ieee802154_rx_abort_observation(evidence.rx_abort_reason()),
            polls: evidence.polls(),
        },
        Ieee802154PolledOperationFailure::Timeout { polls, .. } => {
            Ieee802154PolledEdOutcome::Timeout { polls }
        }
        Ieee802154PolledOperationFailure::CpuInterruptRouteAttached { stage } => {
            Ieee802154PolledEdOutcome::CpuInterruptRouteAttached {
                stage: map_ieee802154_polled_stage(stage),
            }
        }
        Ieee802154PolledOperationFailure::UnexpectedEventMask { stage, observed } => {
            Ieee802154PolledEdOutcome::UnexpectedEventMask {
                stage: map_ieee802154_polled_stage(stage),
                observed: map_ieee802154_event_mask(observed),
            }
        }
        Ieee802154PolledOperationFailure::UnexpectedRxAbortMask { stage, observed } => {
            Ieee802154PolledEdOutcome::UnexpectedRxAbortMask {
                stage: map_ieee802154_polled_stage(stage),
                observed: map_ieee802154_rx_abort_mask(observed),
            }
        }
        Ieee802154PolledOperationFailure::StaleEventStatus { observed } => {
            Ieee802154PolledEdOutcome::StaleEventStatus {
                event_status: map_ieee802154_observed_events(observed.state()),
            }
        }
        Ieee802154PolledOperationFailure::UnexpectedTerminalStatus { observed } => {
            Ieee802154PolledEdOutcome::UnexpectedTerminalStatus {
                event_status: map_ieee802154_observed_events(observed.state()),
            }
        }
        Ieee802154PolledOperationFailure::UnexpectedAcknowledgedEvents { observed } => {
            Ieee802154PolledEdOutcome::UnexpectedAcknowledgedEvents {
                event_status: map_ieee802154_observed_events(observed.state()),
            }
        }
        Ieee802154PolledOperationFailure::ConflictingTerminalEvents { observed } => {
            Ieee802154PolledEdOutcome::ConflictingTerminalEvents {
                event_status: map_ieee802154_observed_events(observed.state()),
            }
        }
    }
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_polled_ed_success(
    evidence: Ieee802154PolledOperationEvidence,
) -> Option<Ieee802154PolledEdOutcome> {
    match evidence.result() {
        Ieee802154PolledOperationResult::EnergyDetection { rss_code } => {
            Some(Ieee802154PolledEdOutcome::Complete {
                rss_code,
                polls: evidence.polls(),
            })
        }
        Ieee802154PolledOperationResult::ClearChannelAssessment { .. } => None,
    }
}

#[cfg(feature = "i-ed-event-probe")]
fn failed_ieee802154_production_ed(
    first: Ieee802154PolledEdOutcome,
    second: Option<Ieee802154PolledEdOutcome>,
) -> Ieee802154EdEventProbeEvidence {
    let mut evidence = unsupported_ieee802154_ed_event_probe();
    evidence.stop = Ieee802154EdEventProbeStop::ProductionEdFailed;
    evidence.production_ed_first = first;
    evidence.production_ed_second = second;
    evidence
}

#[cfg(feature = "i-ed-event-probe")]
const fn map_ieee802154_ed_event_probe(
    evidence: HalIeee802154EdEventProbeEvidence,
) -> Ieee802154EdEventProbeEvidence {
    let stop = match evidence.stop {
        HalIeee802154EdEventProbeStop::Complete => Ieee802154EdEventProbeStop::Complete,
        HalIeee802154EdEventProbeStop::UnsupportedSetup => {
            Ieee802154EdEventProbeStop::UnsupportedSetup
        }
        HalIeee802154EdEventProbeStop::RouteNotQuiesced => {
            Ieee802154EdEventProbeStop::RouteNotQuiesced
        }
        HalIeee802154EdEventProbeStop::ResetNotClear => Ieee802154EdEventProbeStop::ResetNotClear,
        HalIeee802154EdEventProbeStop::EdDurationReadbackMismatch => {
            Ieee802154EdEventProbeStop::EdDurationReadbackMismatch
        }
        HalIeee802154EdEventProbeStop::EventEnableReadbackMismatch => {
            Ieee802154EdEventProbeStop::EventEnableReadbackMismatch
        }
        HalIeee802154EdEventProbeStop::RxAbortEnableReadbackMismatch => {
            Ieee802154EdEventProbeStop::RxAbortEnableReadbackMismatch
        }
        HalIeee802154EdEventProbeStop::PostEnableStatusNotClear => {
            Ieee802154EdEventProbeStop::PostEnableStatusNotClear
        }
        HalIeee802154EdEventProbeStop::TimerActivityTimeout => {
            Ieee802154EdEventProbeStop::TimerActivityTimeout
        }
        HalIeee802154EdEventProbeStop::PairLatchTimeout => {
            Ieee802154EdEventProbeStop::PairLatchTimeout
        }
        HalIeee802154EdEventProbeStop::EdAborted => Ieee802154EdEventProbeStop::EdAborted,
        HalIeee802154EdEventProbeStop::UnexpectedEvent => {
            Ieee802154EdEventProbeStop::UnexpectedEvent
        }
        HalIeee802154EdEventProbeStop::SelectiveWriteMismatch => {
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch
        }
        HalIeee802154EdEventProbeStop::CleanupNotClear => {
            Ieee802154EdEventProbeStop::CleanupNotClear
        }
    };
    Ieee802154EdEventProbeEvidence {
        stop,
        production_ed_first: Ieee802154PolledEdOutcome::NotRun,
        production_ed_second: None,
        event_enable_before: map_ieee802154_validation_event_enable(evidence.event_enable_before),
        event_enable_active: map_ieee802154_validation_event_enable(evidence.event_enable_active),
        event_enable_after: map_ieee802154_validation_event_enable(evidence.event_enable_after),
        rx_abort_enable_before: map_ieee802154_validation_rx_abort_enable(
            evidence.rx_abort_enable_before,
        ),
        rx_abort_enable_active: map_ieee802154_validation_rx_abort_enable(
            evidence.rx_abort_enable_active,
        ),
        rx_abort_enable_after: map_ieee802154_validation_rx_abort_enable(
            evidence.rx_abort_enable_after,
        ),
        ed_duration_before: map_ieee802154_validation_ed_duration(evidence.ed_duration_before),
        ed_duration_active: map_ieee802154_validation_ed_duration(evidence.ed_duration_active),
        ed_duration_after: map_ieee802154_validation_ed_duration(evidence.ed_duration_after),
        timer0_value_before_start: evidence.timer0_value_before_start,
        timer0_value_min: evidence.timer0_value_min,
        timer0_value_max: evidence.timer0_value_max,
        timer0_value_after_stop: evidence.timer0_value_after_stop,
        reset_events: map_ieee802154_observed_events(evidence.reset_events),
        post_enable_events: map_ieee802154_observed_events(evidence.post_enable_events),
        observed_events: map_ieee802154_observed_events(evidence.observed_events),
        terminal_events: map_ieee802154_observed_events(evidence.terminal_events),
        after_ed_done_write_events: map_ieee802154_observed_events(
            evidence.after_ed_done_write_events,
        ),
        after_timer0_write_events: map_ieee802154_observed_events(
            evidence.after_timer0_write_events,
        ),
        cleanup_pending_events: map_ieee802154_observed_events(evidence.cleanup_pending_events),
        final_events: map_ieee802154_observed_events(evidence.final_events),
        rx_abort_reason: match evidence.rx_abort_reason {
            Some(observation) => Some(map_ieee802154_rx_abort_observation(observation)),
            None => None,
        },
        stop_command_issued: evidence.stop_command_issued,
        cleanup_clear: evidence.cleanup_clear,
    }
}

/// Run the bounded ED-DONE/TIMER0 discriminator without installing a CPU IRQ
/// route or exposing validation-only status writes to production code.
#[cfg(feature = "i-ed-event-probe")]
pub(super) fn run_ed_event_probe(
    platform: EspHalRadioPlatform,
    request: Ieee802154EdEventProbeRequest,
) -> Ieee802154EdEventProbeEvidence {
    let Some(config) =
        Ieee802154EdEventProbeConfig::new(request.poll_limit, request.timer_threshold)
    else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let Some(isolation) = Ieee802154EdEventProbeIsolation::claim_for_reset_isolated_image() else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let Some(foundation) = ieee802154_foundation(platform) else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let Ok(channel) = Ieee802154Channel::new(11) else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let policy = Ieee802154MacPolicy::new(
        channel,
        Ieee802154CcaMode::EnergyDetection,
        0,
        Ieee802154AckTimeout::from_units(0),
        Ieee802154MacControl::new(false, false, false, false, false, false),
        Ieee802154PanIdentity::new(0, 0, [0; 8]),
    );
    let Ok(policy_configured) = foundation.configure_mac_policy(policy) else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let Some(budget) = Ieee802154OperationPollBudget::new(request.poll_limit) else {
        return unsupported_ieee802154_ed_event_probe();
    };
    let first = match policy_configured.energy_detection_raw(8, budget) {
        Ok(completed) => completed,
        Err(failed) => {
            return failed_ieee802154_production_ed(
                map_ieee802154_polled_failure(failed.failure()),
                None,
            );
        }
    };
    let Some(first_evidence) = map_ieee802154_polled_ed_success(*first.evidence()) else {
        return failed_ieee802154_production_ed(Ieee802154PolledEdOutcome::NotRun, None);
    };
    let second = match first.into_owner().energy_detection_raw(8, budget) {
        Ok(completed) => completed,
        Err(failed) => {
            return failed_ieee802154_production_ed(
                first_evidence,
                Some(map_ieee802154_polled_failure(failed.failure())),
            );
        }
    };
    let Some(second_evidence) = map_ieee802154_polled_ed_success(*second.evidence()) else {
        return failed_ieee802154_production_ed(
            first_evidence,
            Some(Ieee802154PolledEdOutcome::NotRun),
        );
    };
    let finished = second
        .into_owner()
        .validation_probe_ed_event_status(config, isolation);
    let mut evidence = map_ieee802154_ed_event_probe(*finished.evidence());
    evidence.production_ed_first = first_evidence;
    evidence.production_ed_second = Some(second_evidence);
    evidence
}
