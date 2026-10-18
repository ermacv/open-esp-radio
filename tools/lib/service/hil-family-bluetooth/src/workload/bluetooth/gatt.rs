//! Independent Linux ATT peer for the shared plaintext Trouble application.
use super::establish::{Attempt, CONFIRMATION, establish};
use crate::{Result, fixture::bluetooth::att, fixture::bluetooth::model::PeerAddress};
use oer_hil_link::SerialCapture;
use oer_hil_protocol::bluetooth::BluetoothGattEvidence as Evidence;
use oer_hil_run_bundle_format::run::{Better, Measurement, MeasurementUnit};
use oer_hil_workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(serde::Serialize)]
pub(super) struct Exchange {
    connection: u32,
    request: Vec<u8>,
    response: Vec<u8>,
}

/// The connections a run cycles through.
const CONNECTIONS: usize = 3;

pub fn run(output: &Path, context: &Context<'_>, trace: bool) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut samples = Vec::new();
        let mut exchanges = Vec::new();
        let mut attempts = Vec::new();
        let mut cycles = Vec::new();
        let mut early_losses = Vec::new();
        let probe = exercise(
            capture,
            &owner,
            &mut samples,
            &mut exchanges,
            &mut attempts,
            &mut early_losses,
            trace.then_some(&mut cycles),
        );
        if trace {
            context.results.observe("bluetooth-trace", &cycles);
            context.measurements.check(
                "bluetooth.trace.connection-steps",
                probe.is_ok() && cycles.len() == CONNECTIONS,
            );
        }
        context.measurements.record(super::establish::measurements(&attempts));
        context.measurements.record([Measurement::observed(
            "connections-lost-before-att",
            early_losses.len() as u64,
            MeasurementUnit::Count,
        )
        .better(Better::Lower)]);
        let cleanup = oer_process::cleanup(|| -> Result<()> {
            owner.restore()?;
            wait(capture, &mut samples, |e| !e.connected)?;
            Ok(())
        });
        context.results.observe("trouble-gatt", &serde_json::json!({
            "schema": 3, "security": "plaintext", "connections": 3,
            "samples": samples, "exchanges": exchanges, "connection_attempts": attempts,
            "lost_before_att": early_losses,
            "passed": probe.is_ok() && cleanup.is_ok(),
            "error": probe.as_ref().err().map(ToString::to_string),
            "restored": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
        }));
        join(probe, cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    join(result, restore)
}

fn join(first: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (first, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}").into()),
    }
}

/// One DUT observation, logged; insufficient stack headroom is an error.
fn sample(capture: &SerialCapture, samples: &mut Vec<Evidence>) -> Result<Evidence> {
    let evidence = crate::link::gatt(capture)?;
    samples.push(evidence);
    if !evidence
        .cpu0_stack
        .is_some_and(|stack| stack.has_required_headroom())
    {
        return Err("missing or insufficient GATT task-stack headroom".into());
    }
    Ok(evidence)
}

fn wait(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    predicate: impl Fn(&Evidence) -> bool,
) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let evidence = sample(capture, samples)?;
        if predicate(&evidence) {
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            return Err(format!("GATT transition deadline: {evidence:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

/// Whether `predicate` holds within `within`; a failed observation is the
/// error, never a `false`.
fn holds_within(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    within: Duration,
    predicate: impl Fn(&Evidence) -> bool,
) -> Result<bool> {
    let deadline = Instant::now() + within;
    loop {
        if predicate(&sample(capture, samples)?) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

fn exchange(
    peer: &att::Att,
    connection: u32,
    request: &[u8],
    exchanges: &mut Vec<Exchange>,
) -> Result<Vec<u8>> {
    peer.send(request)?;
    let response = peer.receive()?;
    exchanges.push(Exchange {
        connection,
        request: request.to_vec(),
        response: response.clone(),
    });
    Ok(response)
}

/// Exchange the ATT MTU and discover the application's value handle by its
/// UUIDs over ATT; no target handle is imported.
pub(super) fn discover(
    peer: &att::Att,
    connection: u32,
    exchanges: &mut Vec<Exchange>,
) -> Result<u16> {
    let mtu = exchange(peer, connection, &[2, 23, 0], exchanges)?;
    if mtu.len() != 3 || mtu[0] != 3 || !(23..=517).contains(&word(&mtu[1..])) {
        return Err(format!("invalid ATT MTU response: {mtu:?}").into());
    }
    let service = service(&exchange(
        peer,
        connection,
        &[6, 1, 0, 255, 255, 0, 0x28, 0xf0, 0xff],
        exchanges,
    )?)?;
    let [start_low, start_high] = service.0.to_le_bytes();
    let [end_low, end_high] = service.1.to_le_bytes();
    characteristic(
        &exchange(
            peer,
            connection,
            &[8, start_low, start_high, end_low, end_high, 3, 0x28],
            exchanges,
        )?,
        service,
    )
}

fn exercise(
    capture: &SerialCapture,
    owner: &att::Owner,
    samples: &mut Vec<Evidence>,
    exchanges: &mut Vec<Exchange>,
    attempts: &mut Vec<Attempt>,
    early_losses: &mut Vec<u32>,
    mut traced: Option<&mut Vec<CycleSteps>>,
) -> Result<()> {
    if !capture
        .request_image_keys(Duration::from_secs(10))?
        .has::<oer_hil_protocol::bluetooth::Gatt>()
    {
        return Err("exclusive Trouble GATT image required".into());
    }
    let initial = wait(capture, samples, |e| e.address.is_some() && e.advertising)?;
    if initial.connections != 0
        || initial.disconnections != 0
        || initial.writes != 0
        || initial.value != 0
    {
        return Err("GATT requires a fresh application epoch".into());
    }
    let address = initial.address.ok_or("missing Controller address")?;
    let mut previous = 0;
    for (index, written) in [0x31, 0x52, 0x73].into_iter().enumerate() {
        let connection = index as u32 + 1;
        if traced.is_some() {
            start_trace(capture)?;
        }
        let cycle = Cycle {
            connection,
            address,
            previous,
            written,
        };
        if let Err(error) = cycle.run(capture, owner, samples, exchanges, attempts, early_losses) {
            if traced.is_some() {
                // The post-mortem drains the failed cycle's trace; recording
                // stops so that advertising does not overwrite it.
                let _ = control(
                    capture,
                    oer_hil_protocol::telemetry::TraceControl::SetMask { mask: 0 },
                );
            }
            return Err(error);
        }
        if let Some(cycles) = traced.as_deref_mut() {
            let steps = drain_cycle(capture)?;
            let complete = steps.complete();
            cycles.push(steps);
            if !complete {
                return Err(format!(
                    "connection {connection}'s trace misses a step: {:?}",
                    cycles.last()
                )
                .into());
            }
        }
        previous = written;
    }
    Ok(())
}

/// Whether the DUT reports connection `connection` open.
fn confirms(e: &Evidence, connection: u32) -> bool {
    e.connected && e.connections == connection
}

/// Whether the DUT never saw connection `connection`: it still counts the
/// previous ones, is not connected and advertises.
fn unseen(e: &Evidence, connection: u32) -> bool {
    e.connections == connection - 1 && !e.connected && e.advertising
}

fn validate_progress(e: &Evidence, connection: u32, value: u8, address: [u8; 6]) -> Result<()> {
    if !e.connected
        || e.advertising
        || e.connections != connection
        || e.disconnections != connection - 1
        || e.writes != connection
        || e.reads < 2 * connection
        || e.value != value
        || e.address != Some(address)
    {
        return Err(format!("ATT peer and application observations disagree: {e:?}").into());
    }
    Ok(())
}

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn service(response: &[u8]) -> Result<(u16, u16)> {
    if response.len() != 5 || response[0] != 7 {
        return Err("expected exactly one ATT service range".into());
    }
    let range = (word(&response[1..]), word(&response[3..]));
    if range.0 == 0 || range.0 >= range.1 {
        return Err("invalid ATT service range".into());
    }
    Ok(range)
}
fn characteristic(response: &[u8], service: (u16, u16)) -> Result<u16> {
    if response.len() != 9
        || response[..2] != [9, 7]
        || response[4] != 0x0a
        || word(&response[7..]) != 0xfff1
    {
        return Err("expected one read/write application characteristic".into());
    }
    let declaration = word(&response[2..]);
    let value = word(&response[5..]);
    if declaration <= service.0 || declaration >= value || value > service.1 {
        return Err("characteristic handle outside discovered service".into());
    }
    Ok(value)
}

/// One connection of the run: open it, read the value the previous one
/// wrote, write its own, read it back and close it gracefully.
struct Cycle {
    connection: u32,
    address: [u8; 6],
    previous: u8,
    written: u8,
}

impl Cycle {
    fn run(
        &self,
        capture: &SerialCapture,
        owner: &att::Owner,
        samples: &mut Vec<Evidence>,
        exchanges: &mut Vec<Exchange>,
        attempts: &mut Vec<Attempt>,
        early_losses: &mut Vec<u32>,
    ) -> Result<()> {
        let Self {
            connection,
            address,
            ..
        } = *self;
        let peer = establish(
            "connect",
            connection,
            attempts,
            samples,
            |_| owner.connect(PeerAddress(address)),
            |samples| holds_within(capture, samples, CONFIRMATION, |e| confirms(e, connection)),
            // Dropping the socket closes the host's side.
            |_, peer| {
                drop(peer);
                Ok(())
            },
            |samples| Ok(unseen(&sample(capture, samples)?, connection)),
        )?;
        let served = serve(capture, samples, exchanges, peer, self);
        served.map_err(
            |error| match lost_early(capture, samples, exchanges, connection) {
                Ok(true) => {
                    early_losses.push(connection);
                    format!(
                        "connection {connection} was lost by supervision timeout (0x08) before \
                         its first ATT exchange: {error}"
                    )
                    .into()
                }
                Ok(false) => error,
                Err(observation) => {
                    format!("{error} (then observing the DUT failed: {observation})").into()
                }
            },
        )
    }
}

/// Serve one confirmed connection until its graceful close.
fn serve(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    exchanges: &mut Vec<Exchange>,
    peer: att::Att,
    cycle: &Cycle,
) -> Result<()> {
    let Cycle {
        connection,
        address,
        previous,
        written,
    } = *cycle;
    let live = wait(capture, samples, |e| {
        e.connected && e.connections == connection
    })?;
    if live.address != Some(address) || live.disconnections != connection - 1 {
        return Err("GATT connection changed its application epoch".into());
    }
    let handle = discover(&peer, connection, exchanges)?;
    let [low, high] = handle.to_le_bytes();
    if exchange(&peer, connection, &[0x0a, low, high], exchanges)? != [0x0b, previous] {
        return Err("GATT value did not survive reconnect".into());
    }
    if exchange(&peer, connection, &[0x12, low, high, written], exchanges)? != [0x13] {
        return Err("ATT write was not acknowledged".into());
    }
    if exchange(&peer, connection, &[0x0a, low, high], exchanges)? != [0x0b, written] {
        return Err("ATT read did not return the written value".into());
    }
    let progressed = wait(capture, samples, |e| {
        e.writes >= connection && e.reads >= 2 * connection
    })?;
    validate_progress(&progressed, connection, written, address)?;
    drop(peer);
    let idle = wait(capture, samples, |e| {
        !e.connected && e.advertising && e.disconnections == connection
    })?;
    if idle.last_disconnect_reason != Some(0x13)
        || idle.connections != connection
        || idle.advertising_starts != connection + 1
        || idle.address != Some(address)
    {
        return Err(format!("expected graceful same-epoch re-advertising: {idle:?}").into());
    }
    Ok(())
}

/// Whether the failed `connection` ended as  saw it: the DUT reports it
/// closed by supervision timeout before any ATT exchange on it. The DUT may
/// report the closing after the host's failure, so it gets [`CONFIRMATION`].
fn lost_early(
    capture: &SerialCapture,
    samples: &mut Vec<Evidence>,
    exchanges: &[Exchange],
    connection: u32,
) -> Result<bool> {
    holds_within(capture, samples, CONFIRMATION, |e| {
        e.disconnections >= connection
    })?;
    Ok(samples
        .last()
        .is_some_and(|state| lost_before_att(state, connection, exchanges)))
}

/// Supervision timeout (0x08) ended `connection` before its first ATT
/// exchange.
fn lost_before_att(state: &Evidence, connection: u32, exchanges: &[Exchange]) -> bool {
    state.disconnections == connection
        && state.last_disconnect_reason == Some(0x08)
        && !exchanges
            .iter()
            .any(|exchange| exchange.connection == connection)
}

/// The steps one connection cycle left in the drained trace, oldest first.
#[derive(Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(super) struct CycleSteps {
    /// The indication was accepted, before the opening.
    accepted: bool,
    /// The connection opened.
    opened: bool,
    /// Connection events the radio role admitted while it was open.
    admitted_events: usize,
    /// Connection events that ended with an anchor from a peer packet.
    anchored_events: usize,
    /// The connection closed, after its events.
    closed: bool,
    /// Every connection indication verdict.
    verdicts: Vec<String>,
    /// Bluetooth LE trace records read.
    records: usize,
}

impl CycleSteps {
    fn complete(&self) -> bool {
        self.accepted
            && self.opened
            && self.admitted_events > 0
            && self.anchored_events > 0
            && self.closed
    }
}

/// Record a cycle's steps with the radio role's admissions and event ends:
/// each connection event adds two records, so a cycle fits the trace,
/// where the per-step connection-event channel would wrap it. The role's
/// advertising-channel PDUs show whether a lost indication reached the
/// radio; a connection adds none.
fn start_trace(capture: &SerialCapture) -> Result<()> {
    use oer_bluetooth_trace::{
        ConnectionIndication, Lifecycle, PeripheralClosed, PeripheralOpened, RoleEventEnd, RolePdu,
        RoleRequest,
    };
    use oer_trace::Event as _;
    let mask = RoleRequest::CHANNEL.mask()
        | RoleEventEnd::CHANNEL.mask()
        | RolePdu::CHANNEL.mask()
        | ConnectionIndication::CHANNEL.mask()
        | PeripheralOpened::CHANNEL.mask()
        | PeripheralClosed::CHANNEL.mask()
        | Lifecycle::CHANNEL.mask();
    let status = control(
        capture,
        oer_hil_protocol::telemetry::TraceControl::Start { mask },
    )?;
    if !status.installed || !status.running {
        return Err(format!("the Bluetooth LE trace did not start: {status:?}").into());
    }
    Ok(())
}

fn control(
    capture: &SerialCapture,
    control: oer_hil_protocol::telemetry::TraceControl,
) -> Result<oer_hil_protocol::telemetry::TraceStatus> {
    Ok(capture
        .request(
            0,
            oer_hil_protocol::telemetry::ControlTrace(control),
            Duration::from_secs(5),
        )?
        .0)
}

/// Read the trace of one cycle; a trace that filled up lost its oldest
/// steps, which is an error, never a missing step. Advertising goes on after
/// the disconnection, so recording stops first: the check then holds for
/// every entry read.
fn drain_cycle(capture: &SerialCapture) -> Result<CycleSteps> {
    let status = control(
        capture,
        oer_hil_protocol::telemetry::TraceControl::SetMask { mask: 0 },
    )?;
    if status.stored_entries >= status.entries {
        return Err(format!("the cycle's trace filled up: {status:?}").into());
    }
    Ok(cycle_steps(&capture.trace_entries(status.entries)?))
}

/// The steps of one cycle: an accepted indication, then the opening, then
/// admitted connection events and their anchored ends, then the closing.
pub(super) fn cycle_steps(entries: &[oer_hil_protocol::telemetry::TraceEntry]) -> CycleSteps {
    use oer_bluetooth_trace::{
        ConnectionIndication, EndResult, IndicationVerdict, PeripheralClosed, PeripheralOpened,
        RequestAnswer, RequestKind, RoleEventEnd, RoleRequest,
    };
    use oer_trace::Event as _;
    let mut records = entries
        .iter()
        .map(|entry| oer_trace::Record {
            tag: entry.tag,
            kind: entry.kind,
            t_us: entry.t_us,
            words: entry.words,
        })
        .collect::<Vec<_>>();
    oer_trace::oldest_first(&mut records);
    let mut steps = CycleSteps::default();
    for record in records {
        if oer_trace::Kind::from_raw(record.kind)
            .is_some_and(|kind| kind.domain == oer_trace::Domain::Bluetooth)
        {
            steps.records += 1;
        }
        let open = steps.opened && !steps.closed;
        if let Some(indication) = (record.kind == ConnectionIndication::KIND.raw())
            .then(|| ConnectionIndication::decode(record.words))
            .flatten()
        {
            steps.verdicts.push(format!("{:?}", indication.verdict));
            steps.accepted |= indication.verdict == IndicationVerdict::Accepted;
        } else if record.kind == PeripheralOpened::KIND.raw() {
            steps.opened = steps.accepted;
        } else if let Some(request) = (open && record.kind == RoleRequest::KIND.raw())
            .then(|| RoleRequest::decode(record.words))
            .flatten()
        {
            if request.kind == RequestKind::ConnectionEvent
                && request.answer == RequestAnswer::Accepted
            {
                steps.admitted_events += 1;
            }
        } else if let Some(end) = (open && record.kind == RoleEventEnd::KIND.raw())
            .then(|| RoleEventEnd::decode(record.words))
            .flatten()
        {
            if end.result == EndResult::ExecutedWithAnchor {
                steps.anchored_events += 1;
            }
        } else if record.kind == PeripheralClosed::KIND.raw() {
            steps.closed = open && steps.anchored_events > 0;
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_supervision_timeout_before_any_exchange_is_an_early_loss() {
        let state = |disconnections, reason| Evidence {
            disconnections,
            last_disconnect_reason: reason,
            ..Evidence::default()
        };
        let exchanged = [super::Exchange {
            connection: 2,
            request: vec![0x02, 23, 0],
            response: vec![0x03, 23, 0],
        }];
        assert!(super::lost_before_att(&state(1, Some(0x08)), 1, &exchanged));
        // Another reason, an earlier connection's closing, or an exchange
        // on the connection is something else.
        assert!(!super::lost_before_att(
            &state(1, Some(0x13)),
            1,
            &exchanged
        ));
        assert!(!super::lost_before_att(
            &state(1, Some(0x08)),
            2,
            &exchanged
        ));
        assert!(!super::lost_before_att(
            &state(2, Some(0x08)),
            2,
            &exchanged
        ));
    }

    #[test]
    fn a_cycle_needs_every_step_in_order() {
        use oer_bluetooth_trace::{
            ConnectionIndication, EndResult, IndicationVerdict, PeripheralClosed, PeripheralOpened,
            RequestAnswer, RequestKind, RoleEventEnd, RoleRequest,
        };
        use oer_trace::Event;
        fn entries(
            events: &[(oer_trace::Kind, [u32; 2])],
        ) -> Vec<oer_hil_protocol::telemetry::TraceEntry> {
            let mut entries = events
                .iter()
                .enumerate()
                .map(
                    |(index, (kind, words))| oer_hil_protocol::telemetry::TraceEntry {
                        tag: index as u16 + 1,
                        kind: kind.raw(),
                        t_us: index as u32,
                        words: *words,
                    },
                )
                .collect::<Vec<_>>();
            // Storage order is not time order.
            entries.reverse();
            entries
        }
        let accepted = (
            ConnectionIndication::KIND,
            ConnectionIndication {
                verdict: IndicationVerdict::Accepted,
            }
            .encode(),
        );
        let opened = (
            PeripheralOpened::KIND,
            PeripheralOpened {
                first_anchor_low: 1,
                interval_units: 24,
            }
            .encode(),
        );
        let admitted = (
            RoleRequest::KIND,
            RoleRequest {
                kind: RequestKind::ConnectionEvent,
                answer: RequestAnswer::Accepted,
                lead_micros: Some(500),
            }
            .encode(),
        );
        let anchored = (
            RoleEventEnd::KIND,
            RoleEventEnd {
                id: 9,
                result: EndResult::ExecutedWithAnchor,
                received: 1,
                discarded: 0,
            }
            .encode(),
        );
        let closed = (
            PeripheralClosed::KIND,
            PeripheralClosed { reason: Some(0x13) }.encode(),
        );
        let whole = super::cycle_steps(&entries(&[accepted, opened, admitted, anchored, closed]));
        assert!(whole.complete(), "{whole:?}");
        assert_eq!(whole.records, 5);
        assert_eq!(whole.verdicts, ["Accepted"]);
        // Events before the opening do not count; a closing before an
        // anchored end does not close the cycle.
        let early = super::cycle_steps(&entries(&[admitted, anchored, accepted, opened, closed]));
        assert!(!early.complete(), "{early:?}");
        assert_eq!((early.admitted_events, early.anchored_events), (0, 0));
        assert!(!early.closed);
        // An opening without an accepted indication opens nothing.
        let unaccepted = super::cycle_steps(&entries(&[opened, admitted, anchored, closed]));
        assert!(!unaccepted.opened && !unaccepted.complete());
    }

    use super::*;
    #[test]
    fn discovery_rejects_truncation_duplicates_and_foreign_handles() {
        assert_eq!(service(&[7, 8, 0, 10, 0]).unwrap(), (8, 10));
        for response in [
            &[][..],
            &[7, 8, 0],
            &[7, 0, 0, 2, 0],
            &[7, 8, 0, 7, 0],
            &[7, 8, 0, 10, 0, 11, 0, 12, 0],
        ] {
            assert!(service(response).is_err());
        }
        let valid = [9, 7, 9, 0, 0x0a, 10, 0, 0xf1, 0xff];
        assert_eq!(characteristic(&valid, (8, 10)).unwrap(), 10);
        for length in 0..valid.len() {
            assert!(characteristic(&valid[..length], (8, 10)).is_err());
        }
        assert!(characteristic(&valid, (10, 15)).is_err());
        assert!(characteristic(&valid, (8, 9)).is_err());
        let mut wrong = valid;
        wrong[7] = 0xf2;
        assert!(characteristic(&wrong, (8, 10)).is_err());
    }
    #[test]
    fn only_a_connection_the_dut_never_counted_is_unseen() {
        let advertising = Evidence {
            advertising: true,
            connections: 1,
            disconnections: 1,
            ..Evidence::default()
        };
        assert!(unseen(&advertising, 2));
        assert!(!unseen(
            &Evidence {
                connections: 2,
                ..advertising
            },
            2
        ));
        assert!(!unseen(
            &Evidence {
                connected: true,
                ..advertising
            },
            2
        ));
        assert!(!unseen(
            &Evidence {
                advertising: false,
                ..advertising
            },
            2
        ));
    }

    #[test]
    fn only_the_connection_itself_open_on_the_dut_confirms_it() {
        let open = Evidence {
            connected: true,
            connections: 2,
            disconnections: 1,
            ..Evidence::default()
        };
        assert!(confirms(&open, 2));
        assert!(!confirms(
            &Evidence {
                connections: 1,
                ..open
            },
            2
        ));
        assert!(!confirms(
            &Evidence {
                connected: false,
                ..open
            },
            2
        ));
    }

    #[test]
    fn peer_success_cannot_hide_application_restart_or_missing_delivery() {
        let good = Evidence {
            address: Some([1; 6]),
            connected: true,
            connections: 2,
            disconnections: 1,
            reads: 4,
            writes: 2,
            value: 0x52,
            ..Evidence::default()
        };
        assert!(validate_progress(&good, 2, 0x52, [1; 6]).is_ok());
        for bad in [
            Evidence {
                connections: 1,
                ..good
            },
            Evidence { reads: 3, ..good },
            Evidence { writes: 1, ..good },
            Evidence { value: 0, ..good },
            Evidence {
                connected: false,
                ..good
            },
            Evidence {
                address: Some([2; 6]),
                ..good
            },
        ] {
            assert!(validate_progress(&bad, 2, 0x52, [1; 6]).is_err());
        }
    }
}
