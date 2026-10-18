//! Bounded host validation for the IEEE 802.15.4 ED-DONE/TIMER0 probe.
//!
//! The accepted result observes one exact selected-write relation in a
//! reset-isolated, route-detached transaction. It does not classify the full
//! `EVENT_STATUS` register or claim operational PHY/RF/BTBB readiness.

use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use std::{path::Path, time::Duration};

use oer_hil_protocol::{
    ieee802154::Ieee802154EdEventProbeEvidence, ieee802154::Ieee802154EdEventProbeRequest,
    ieee802154::Ieee802154EdEventProbeStop, ieee802154::Ieee802154ObservedEventState,
    ieee802154::Ieee802154PolledEdOutcome, ieee802154::Ieee802154ValidationEdDurationState,
    ieee802154::Ieee802154ValidationEventEnableState,
    ieee802154::Ieee802154ValidationRxAbortEnableState,
};
#[cfg(test)]
use oer_hil_protocol::{
    ieee802154::Ieee802154RxAbortObservation, ieee802154::Ieee802154RxAbortReason,
};

use crate::Result;
use oer_hil_link::SerialCapture;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const RESULT: &str = "back-to-back-production-ed-and-selected-write-recovery";

pub struct Config {
    pub boots: u8,
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

/// What a passing probe does not establish.
const NOT_PROVEN: &[&str] = &[
    "full-register-w1c-semantics",
    "non-ed-event-write-semantics",
    "concurrent-same-bit-arrival",
    "level-triggered-route-behavior",
    "production-phy-rf-btbb-readiness",
    "synchronous-stop-semantics",
    "calibrated-rss-or-dbm-conversion",
    "rf-channel-retune",
];

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let request = Ieee802154EdEventProbeRequest {
        poll_limit: config.poll_limit,
        timer_threshold: config.timer_threshold,
    };
    if !request.validate() {
        return Err("invalid IEEE 802.15.4 ED event probe bounds".into());
    }
    context.results.claim(RESULT, NOT_PROVEN);
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| probe(capture, request),
        |_, target| validate(*target, config.poll_limit),
    )?;
    println!(
        "ieee802154_ed_event=PASS result={RESULT} boots={}",
        config.boots
    );
    Ok(())
}

fn probe(
    capture: &SerialCapture,
    request: Ieee802154EdEventProbeRequest,
) -> Result<Ieee802154EdEventProbeEvidence> {
    require_keys::<oer_hil_protocol::ieee802154::EdEventProbe>(capture)?;
    let oer_hil_protocol::ieee802154::EdEventProbed(evidence) = capture.request(
        0,
        oer_hil_protocol::ieee802154::ProbeEdEvent(request),
        COMMAND_TIMEOUT,
    )?;
    Ok(evidence)
}

fn validate(evidence: Ieee802154EdEventProbeEvidence, poll_limit: u32) -> Result<()> {
    let validate_production =
        |attempt: &str, outcome: Ieee802154PolledEdOutcome| -> Result<()> {
            match outcome {
        Ieee802154PolledEdOutcome::Complete { polls, .. } if polls >= 1 && polls <= poll_limit => {
            Ok(())
        }
        Ieee802154PolledEdOutcome::Complete { polls, .. } => Err(format!(
            "IEEE 802.15.4 {attempt} production ED used {polls} polls outside 1..={poll_limit}"
        )
        .into()),
        failure => Err(format!("IEEE 802.15.4 {attempt} production ED failed: {failure:?}").into()),
            }
        };
    validate_production("first", evidence.production_ed_first)?;
    let Some(second) = evidence.production_ed_second else {
        return Err("IEEE 802.15.4 second production ED was not run".into());
    };
    validate_production("second", second)?;

    if evidence.stop != Ieee802154EdEventProbeStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 ED event probe stopped at {:?}: {evidence:?}",
            evidence.stop,
        )
        .into());
    }
    for (checkpoint, observed, expected) in [
        (
            "event_enable_before",
            evidence.event_enable_before,
            Ieee802154ValidationEventEnableState::AllMasked,
        ),
        (
            "event_enable_active",
            evidence.event_enable_active,
            Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly,
        ),
        (
            "event_enable_after",
            evidence.event_enable_after,
            Ieee802154ValidationEventEnableState::AllMasked,
        ),
    ] {
        if observed != expected {
            return semantic_checkpoint_error(checkpoint, observed, expected);
        }
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
            "observed_events",
            evidence.observed_events,
            Ieee802154ObservedEventState::EdDoneAndTimer0,
        ),
        (
            "terminal_events",
            evidence.terminal_events,
            Ieee802154ObservedEventState::EdDoneAndTimer0,
        ),
        (
            "after_ed_done_write_events",
            evidence.after_ed_done_write_events,
            Ieee802154ObservedEventState::Timer0Only,
        ),
        (
            "after_timer0_write_events",
            evidence.after_timer0_write_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "cleanup_pending_events",
            evidence.cleanup_pending_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "final_events",
            evidence.final_events,
            Ieee802154ObservedEventState::Clear,
        ),
    ] {
        if observed != expected {
            return semantic_checkpoint_error(checkpoint, observed, expected);
        }
    }

    if evidence.rx_abort_enable_active
        != Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly
    {
        return semantic_checkpoint_error(
            "rx_abort_enable_active",
            evidence.rx_abort_enable_active,
            Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly,
        );
    }
    if evidence.rx_abort_enable_before != Ieee802154ValidationRxAbortEnableState::AllMasked {
        return semantic_checkpoint_error(
            "rx_abort_enable_before",
            evidence.rx_abort_enable_before,
            Ieee802154ValidationRxAbortEnableState::AllMasked,
        );
    }
    if evidence.rx_abort_enable_after != evidence.rx_abort_enable_before {
        return semantic_checkpoint_error(
            "rx_abort_enable_after",
            evidence.rx_abort_enable_after,
            evidence.rx_abort_enable_before,
        );
    }
    if evidence.ed_duration_active != Ieee802154ValidationEdDurationState::ValidationEight {
        return semantic_checkpoint_error(
            "ed_duration_active",
            evidence.ed_duration_active,
            Ieee802154ValidationEdDurationState::ValidationEight,
        );
    }
    if evidence.ed_duration_after != Ieee802154ValidationEdDurationState::ValidationEight {
        return semantic_checkpoint_error(
            "ed_duration_after",
            evidence.ed_duration_after,
            Ieee802154ValidationEdDurationState::ValidationEight,
        );
    }

    // The HAL terminal state proves only the selected-write relation. HIL
    // additionally qualifies that TIMER0 showed bounded activity while the
    // paired ED-DONE/TIMER0 image was acquired on this physical target.
    if evidence.timer0_value_min >= evidence.timer0_value_max
        || evidence.timer0_value_before_start < evidence.timer0_value_min
        || evidence.timer0_value_before_start > evidence.timer0_value_max
    {
        return Err(format!(
            "IEEE 802.15.4 ED event TIMER0 did not show bounded activity: before={}, min={}, max={}, after_stop={}",
            evidence.timer0_value_before_start,
            evidence.timer0_value_min,
            evidence.timer0_value_max,
            evidence.timer0_value_after_stop,
        )
        .into());
    }
    if evidence.rx_abort_reason.is_some() {
        return Err(format!(
            "IEEE 802.15.4 ED event probe retained unexpected RX_ABORT reason {:?}",
            evidence.rx_abort_reason
        )
        .into());
    }
    if evidence.stop_command_issued {
        return Err("nominal IEEE 802.15.4 ED event probe issued timeout STOP".into());
    }
    if !evidence.cleanup_clear {
        return Err("IEEE 802.15.4 ED event probe did not prove its closed cleanup".into());
    }
    Ok(())
}

fn semantic_checkpoint_error<T, Observation: core::fmt::Debug>(
    checkpoint: &str,
    observed: Observation,
    expected: Observation,
) -> Result<T> {
    Err(format!("IEEE 802.15.4 ED event {checkpoint}={observed:?}, expected {expected:?}").into())
}

#[cfg(test)]
mod tests;
