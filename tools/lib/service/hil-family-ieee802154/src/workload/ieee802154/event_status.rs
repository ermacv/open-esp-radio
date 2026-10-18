//! Bounded host validation for the IEEE 802.15.4 `EVENT_STATUS` HIL probe.
//!
//! The accepted result observes selective acknowledgement of two enabled
//! timer bits and a later, distinct arrival of the second timer bit while the
//! source-132 CPU routes retain their reset-detached controls. It does not
//! prove full W1C semantics, concurrent arrival of the same bit, or active
//! level-triggered interrupt-route behavior.

use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use std::{path::Path, time::Duration};

use oer_hil_protocol::{
    ieee802154::Ieee802154EventStatusProbeEvidence, ieee802154::Ieee802154EventStatusProbeRequest,
    ieee802154::Ieee802154EventStatusProbeStop, ieee802154::Ieee802154ObservedEventState,
    ieee802154::Ieee802154ValidationEventEnableState,
};

use crate::Result;
use oer_hil_link::SerialCapture;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const RESULT: &str = "route-detached-enabled-selective-ack-and-distinct-arrival-observed";

pub struct Config {
    pub boots: u8,
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

/// What a passing probe does not establish.
const NOT_PROVEN: &[&str] = &[
    "full-w1c-semantics",
    "event-enable-generation-vs-visibility-semantics",
    "concurrent-same-bit-arrival",
    "level-triggered-route-behavior",
    "masked-final-status-means-physical-cleanup",
];

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let request = Ieee802154EventStatusProbeRequest {
        poll_limit: config.poll_limit,
        timer_threshold: config.timer_threshold,
    };
    if !request.validate() {
        return Err("invalid IEEE 802.15.4 EVENT_STATUS probe bounds".into());
    }
    context.results.claim(RESULT, NOT_PROVEN);
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| probe(capture, request),
        |_, target| validate(*target),
    )?;
    eprintln!(
        "ieee802154_event_status=PASS result={RESULT} boots={}",
        config.boots
    );
    Ok(())
}

fn probe(
    capture: &SerialCapture,
    request: Ieee802154EventStatusProbeRequest,
) -> Result<Ieee802154EventStatusProbeEvidence> {
    require_keys::<oer_hil_protocol::ieee802154::EventStatusProbe>(capture)?;
    let oer_hil_protocol::ieee802154::EventStatusProbed(evidence) = capture.request(
        0,
        oer_hil_protocol::ieee802154::ProbeEventStatus(request),
        COMMAND_TIMEOUT,
    )?;
    Ok(evidence)
}

fn validate(evidence: Ieee802154EventStatusProbeEvidence) -> Result<()> {
    if evidence.stop != Ieee802154EventStatusProbeStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 EVENT_STATUS probe stopped at {:?}: {evidence:?}",
            evidence.stop,
        )
        .into());
    }
    if evidence.event_enable_before != Ieee802154ValidationEventEnableState::AllMasked {
        return checkpoint_error(
            "event_enable_before",
            evidence.event_enable_before,
            "be AllMasked",
        );
    }
    if evidence.event_enable_active != Ieee802154ValidationEventEnableState::TimerPairOnly {
        return checkpoint_error(
            "event_enable_active",
            evidence.event_enable_active,
            "be TimerPairOnly",
        );
    }
    if evidence.event_enable_after != Ieee802154ValidationEventEnableState::AllMasked {
        return checkpoint_error(
            "event_enable_after",
            evidence.event_enable_after,
            "be AllMasked",
        );
    }

    for (checkpoint, observed, expected) in [
        (
            "reset_events",
            evidence.reset_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "post_enable_events",
            evidence.post_enable_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "dual_observed_events",
            evidence.dual_observed_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "dual_latched_events",
            evidence.dual_latched_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "after_timer0_ack_events",
            evidence.after_timer0_ack_events,
            Ieee802154ObservedEventState::Timer1Only,
        ),
        (
            "after_timer1_ack_events",
            evidence.after_timer1_ack_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "distinct_snapshot_events",
            evidence.distinct_snapshot_events,
            Ieee802154ObservedEventState::Timer0Only,
        ),
        (
            "distinct_before_ack_events",
            evidence.distinct_before_ack_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "distinct_after_ack_events",
            evidence.distinct_after_ack_events,
            Ieee802154ObservedEventState::Timer1Only,
        ),
        (
            "final_events",
            evidence.final_events,
            Ieee802154ObservedEventState::Clear,
        ),
    ] {
        if observed != expected {
            return checkpoint_error(checkpoint, observed, "equal its exact semantic state");
        }
    }

    if evidence.cleanup_pending_events != Ieee802154ObservedEventState::Clear
        && evidence.cleanup_pending_events != Ieee802154ObservedEventState::Timer1Only
    {
        return checkpoint_error(
            "cleanup_pending_events",
            evidence.cleanup_pending_events,
            "be Clear or Timer1Only after delivery is masked",
        );
    }

    for (timer, before, minimum, maximum) in [
        (
            "timer0",
            evidence.timer0_value_before_start,
            evidence.timer0_value_min,
            evidence.timer0_value_max,
        ),
        (
            "timer1",
            evidence.timer1_value_before_start,
            evidence.timer1_value_min,
            evidence.timer1_value_max,
        ),
    ] {
        if minimum >= maximum || before < minimum || before > maximum {
            return Err(format!(
                "IEEE 802.15.4 EVENT_STATUS {timer} counter did not show bounded activity: before={before}, min={minimum}, max={maximum}"
            )
            .into());
        }
    }
    Ok(())
}

fn checkpoint_error<T, Observation: core::fmt::Debug>(
    checkpoint: &str,
    observed: Observation,
    expected: &str,
) -> Result<T> {
    Err(
        format!("IEEE 802.15.4 EVENT_STATUS {checkpoint}={observed:?}, expected it to {expected}")
            .into(),
    )
}

#[cfg(test)]
mod tests;
