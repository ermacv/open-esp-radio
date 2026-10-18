//! The IEEE 802.15.4 route probe: same-bit arrival and level retrigger of
//! modem source 132 over the production MAC owners and route adapter.
//!
//! The image brings the MAC to its foundation, activates the production
//! interrupt owner and enables TIMER0 overflow beside the runtime event
//! baseline. The polled phase runs with the route unbound. Each routed phase
//! binds a validation handler through the production route adapter at the
//! vendor's priority, raises its first event with TIMER0, lets the handler
//! sample, act and consume its snapshot on every entry, then quiesces the
//! route. The image is terminal.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use oer_esp32s31_hal::ieee802154::{
    Ieee802154Operational, Ieee802154RouteProbeAction, Ieee802154RouteProbeConfig,
    Ieee802154RouteProbeEntry as HalEntry, Ieee802154RouteProbeStop as HalStop,
    Ieee802154SameBitOutcome as HalOutcome, Ieee802154TimerEvents, finish_route_probe,
    ll::Ieee802154MacOwners, route_probe_entry, run_polled_same_bit, start_route_probe_phase,
};
use oer_esp32s31_ieee802154_esp_hal::{bind, now_micros};
use oer_espressif_ieee802154_engine::{ll::Ieee802154LowLevel, types::Ieee802154Event};
use oer_hil_protocol::{
    ieee802154::IEEE802154_ROUTE_PROBE_MAX_ENTRIES, ieee802154::Ieee802154ObservedEventState,
    ieee802154::Ieee802154RouteProbeEntry, ieee802154::Ieee802154RouteProbeEvidence,
    ieee802154::Ieee802154RouteProbeRequest, ieee802154::Ieee802154RouteProbeStop,
    ieee802154::Ieee802154SameBitOutcome,
};

use super::{EspHalRadioPlatform, ieee802154_foundation};

type Entries = heapless::Vec<HalEntry, IEEE802154_ROUTE_PROBE_MAX_ENTRIES>;

/// The owners and record of the running routed phase, shared with the
/// validation handler.
struct Routed {
    owners: Ieee802154MacOwners,
    config: Ieee802154RouteProbeConfig,
    first: Ieee802154RouteProbeAction,
    entries: Entries,
}

static ROUTED: Mutex<CriticalSectionRawMutex, RefCell<Option<Routed>>> =
    Mutex::new(RefCell::new(None));

/// The validation handler of source 132: the first entry of a phase performs
/// its action, every later one only consumes its snapshot. The route-probe
/// image's interrupt table names it.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub(crate) fn route_probe_interrupt() {
    ROUTED.lock(|routed| {
        let mut routed = routed.borrow_mut();
        let Some(routed) = routed.as_mut() else {
            return;
        };
        let action = if routed.entries.is_empty() {
            routed.first
        } else {
            Ieee802154RouteProbeAction::Acknowledge
        };
        let entry = route_probe_entry(&mut routed.owners, action, routed.config, &mut now_micros);
        // Entries beyond the record were still acknowledged.
        let _ = routed.entries.push(entry);
    });
}

const fn events(events: Ieee802154TimerEvents) -> Ieee802154ObservedEventState {
    match events {
        Ieee802154TimerEvents::Clear => Ieee802154ObservedEventState::Clear,
        Ieee802154TimerEvents::Timer0 => Ieee802154ObservedEventState::Timer0Only,
        Ieee802154TimerEvents::Timer1 => Ieee802154ObservedEventState::Timer1Only,
        Ieee802154TimerEvents::Both => Ieee802154ObservedEventState::Timer0AndTimer1,
        Ieee802154TimerEvents::Other => Ieee802154ObservedEventState::UnexpectedNamed,
    }
}

const fn stop(stop: HalStop) -> Ieee802154RouteProbeStop {
    match stop {
        HalStop::Complete => Ieee802154RouteProbeStop::Complete,
        HalStop::NotClear => Ieee802154RouteProbeStop::NotClear,
        HalStop::LatchTimeout => Ieee802154RouteProbeStop::LatchTimeout,
        HalStop::UnexpectedEvent => Ieee802154RouteProbeStop::UnexpectedEvent,
    }
}

const fn outcome(outcome: HalOutcome) -> Ieee802154SameBitOutcome {
    match outcome {
        HalOutcome::NotRun => Ieee802154SameBitOutcome::NotRun,
        HalOutcome::Coalesced => Ieee802154SameBitOutcome::Coalesced,
        HalOutcome::Retained => Ieee802154SameBitOutcome::Retained,
    }
}

fn entries(
    recorded: &Entries,
) -> heapless::Vec<Ieee802154RouteProbeEntry, IEEE802154_ROUTE_PROBE_MAX_ENTRIES> {
    recorded
        .iter()
        .map(|entry| Ieee802154RouteProbeEntry {
            snapshot: events(entry.snapshot),
            before_acknowledgement: events(entry.before_acknowledgement),
        })
        .collect()
}

/// Busy-wait `micros` on the platform clock.
fn wait(micros: u64) {
    let until = now_micros() + micros;
    while now_micros() < until {}
}

/// One routed phase. Returns the owners with the phase's entries, or no
/// owners when the route could not be quiesced.
fn routed_phase(
    mut owners: Ieee802154MacOwners,
    config: Ieee802154RouteProbeConfig,
    first: Ieee802154RouteProbeAction,
) -> (
    Option<Ieee802154MacOwners>,
    Entries,
    Result<(), Ieee802154RouteProbeStop>,
) {
    if let Err(failure) = start_route_probe_phase(&mut owners, config) {
        return (Some(owners), Entries::new(), Err(stop(failure)));
    }
    ROUTED.lock(|routed| {
        routed.borrow_mut().replace(Routed {
            owners,
            config,
            first,
            entries: Entries::new(),
        })
    });
    let take = || {
        ROUTED
            .lock(|routed| routed.borrow_mut().take())
            .map(|routed| (routed.owners, routed.entries))
    };
    let bound = match bind() {
        Ok(bound) => bound,
        Err(_) => {
            let (owners, entries) = take().unzip();
            return (
                owners,
                entries.unwrap_or_default(),
                Err(Ieee802154RouteProbeStop::RouteFailed),
            );
        }
    };
    // The first entry waits up to one settle inside the handler; the later
    // entries need no more than another.
    let settle = u64::from(config.settle_micros());
    wait(u64::from(config.threshold_micros()) + 3 * settle);
    let quiesced = bound.quiesce().is_ok();
    let (owners, entries) = take().unzip();
    let result = if quiesced {
        Ok(())
    } else {
        Err(Ieee802154RouteProbeStop::RouteFailed)
    };
    (owners, entries.unwrap_or_default(), result)
}

/// Run the route probe. The image is terminal.
pub(in crate::product_hil) fn run_route_probe(
    platform: EspHalRadioPlatform,
    request: Ieee802154RouteProbeRequest,
) -> Ieee802154RouteProbeEvidence {
    let mut evidence = Ieee802154RouteProbeEvidence::default();
    let Some(config) =
        Ieee802154RouteProbeConfig::new(request.threshold_micros, request.settle_micros)
    else {
        return evidence;
    };
    let Some(foundation) = ieee802154_foundation(platform) else {
        return evidence;
    };
    let Ieee802154Operational {
        mut task,
        interrupts,
    } = foundation.into_operational();
    let interrupts = interrupts.activate(&mut task);
    let mut owners = Ieee802154MacOwners::new(task, interrupts);
    owners.enable_event(Ieee802154Event::Timer0Overflow);

    let (polled, result) = run_polled_same_bit(&mut owners, config, &mut now_micros);
    evidence.polled_snapshot = events(polled.snapshot);
    evidence.polled_after_acknowledgement = events(polled.after_acknowledgement);
    evidence.polled_control = events(polled.control_rearmed);
    evidence.polled_outcome = outcome(polled.outcome);
    let mut failure = result.err().map(stop);

    let mut owners = Some(owners);
    for (first, record) in [
        (
            Ieee802154RouteProbeAction::RaiseTimer1,
            &mut evidence.retrigger_entries,
        ),
        (
            Ieee802154RouteProbeAction::RefireTimer0,
            &mut evidence.same_bit_entries,
        ),
    ] {
        if failure.is_some() {
            break;
        }
        let Some(current) = owners.take() else {
            break;
        };
        let (returned, recorded, result) = routed_phase(current, config, first);
        *record = entries(&recorded);
        owners = returned;
        failure = failure.or(result.err());
    }

    let Some(mut owners) = owners else {
        evidence.stop = failure.unwrap_or(Ieee802154RouteProbeStop::RouteFailed);
        return evidence;
    };
    let (pending, finished) = finish_route_probe(&mut owners);
    evidence.final_events = events(pending);
    evidence.stop = failure.unwrap_or(stop(finished));
    owners.disable_event(Ieee802154Event::Timer0Overflow);
    let (mut task, interrupts) = owners.into_parts();
    let _ = interrupts.deactivate(&mut task);
    evidence
}
