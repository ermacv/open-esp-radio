//! Host validation of the IEEE 802.15.4 route probe.
//!
//! The probe answers two questions the W1C access model of `EVENT_STATUS`
//! leaves to hardware:
//!
//! 1. same-bit arrival: whether TIMER0 firing again while its bit is still
//!    latched survives the acknowledgement of the first arrival. The polled
//!    phase and the routed phase must give the same answer on every boot;
//!    either answer is a hardware fact the cell records;
//! 2. level retrigger: whether an event that latched after the ISR sampled
//!    its snapshot re-enters the ISR once the snapshot is consumed. The
//!    routed phase must enter exactly twice: TIMER0, then TIMER1.
//!
//! It runs over the production MAC owners and the production route adapter
//! at the vendor's priority, with the MAC timers as the only stimulus.

use std::{path::Path, time::Duration};

use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    ieee802154::Ieee802154ObservedEventState, ieee802154::Ieee802154RouteProbeEntry,
    ieee802154::Ieee802154RouteProbeEvidence, ieee802154::Ieee802154RouteProbeRequest,
    ieee802154::Ieee802154RouteProbeStop, ieee802154::Ieee802154SameBitOutcome,
};
use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};

use crate::Result;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Config {
    pub boots: u8,
    pub threshold_micros: u32,
    pub settle_micros: u32,
}

const fn entry(
    snapshot: Ieee802154ObservedEventState,
    before_acknowledgement: Ieee802154ObservedEventState,
) -> Ieee802154RouteProbeEntry {
    Ieee802154RouteProbeEntry {
        snapshot,
        before_acknowledgement,
    }
}

/// Decide one boot; returns its same-bit outcome.
pub fn evaluate(evidence: &Ieee802154RouteProbeEvidence) -> Result<Ieee802154SameBitOutcome> {
    use Ieee802154ObservedEventState::{Clear, Timer0AndTimer1, Timer0Only, Timer1Only};
    if evidence.stop != Ieee802154RouteProbeStop::Complete {
        return Err(format!("the route probe stopped at {:?}", evidence.stop).into());
    }
    if evidence.polled_snapshot != Timer0Only || evidence.polled_control != Timer0Only {
        return Err(format!(
            "the polled phase sampled {:?} and its control {:?}, expected TIMER0 both times",
            evidence.polled_snapshot, evidence.polled_control
        )
        .into());
    }
    let outcome = evidence.polled_outcome;
    let second_entry = match outcome {
        Ieee802154SameBitOutcome::Coalesced => None,
        Ieee802154SameBitOutcome::Retained => Some(entry(Timer0Only, Timer0Only)),
        Ieee802154SameBitOutcome::NotRun => {
            return Err("the polled phase reached no same-bit outcome".into());
        }
    };
    let retrigger = [
        entry(Timer0Only, Timer0AndTimer1),
        entry(Timer1Only, Timer1Only),
    ];
    if evidence.retrigger_entries.as_slice() != retrigger {
        return Err(format!(
            "the level-retrigger phase entered {:?}, expected {retrigger:?}",
            evidence.retrigger_entries
        )
        .into());
    }
    let same_bit: Vec<_> = std::iter::once(entry(Timer0Only, Timer0Only))
        .chain(second_entry)
        .collect();
    if evidence.same_bit_entries.as_slice() != same_bit.as_slice() {
        return Err(format!(
            "the routed same-bit phase entered {:?}, but the polled phase found {outcome:?}",
            evidence.same_bit_entries
        )
        .into());
    }
    if evidence.final_events != Clear {
        return Err(format!("{:?} stayed latched", evidence.final_events).into());
    }
    Ok(outcome)
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let request = Ieee802154RouteProbeRequest {
        threshold_micros: config.threshold_micros,
        settle_micros: config.settle_micros,
    };
    if !request.validate() {
        return Err("invalid IEEE 802.15.4 route probe timing".into());
    }
    context.results.claim(
        "level-retrigger-observed-and-same-bit-arrival-classified",
        &["radio-event-arrival-races", "multi-core-routes"],
    );
    let mut outcome = None;
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| probe(capture, request),
        |boot, target| {
            let observed = evaluate(target)?;
            match outcome.replace(observed) {
                Some(earlier) if earlier != observed => Err(format!(
                    "boot {boot} found {observed:?}, an earlier boot {earlier:?}"
                )
                .into()),
                _ => Ok(()),
            }
        },
    )?;
    context
        .results
        .observe("same-bit-arrival", &format!("{outcome:?}"));
    eprintln!(
        "ieee802154_route_probe=PASS same_bit={outcome:?} level_retrigger=observed boots={}",
        config.boots
    );
    Ok(())
}

fn probe(
    capture: &SerialCapture,
    request: Ieee802154RouteProbeRequest,
) -> Result<Ieee802154RouteProbeEvidence> {
    require_keys::<oer_hil_protocol::ieee802154::RouteProbe>(capture)?;
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ProbeRoute(request),
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)
}

#[cfg(test)]
mod tests;
