#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! The installation machine over a real port: `drive` carries each step out
//! on a [`DataPort`] through its conformance harness, and the cases every
//! port passes under it, the properties of the boundaries page (section 10)
//! that need no role.
//!
//! A case's pump is a loop over the machine's rests: each turn it lets the
//! fixture's hardware progress, advances the fixture's time to an instant a
//! rest names, serves the port once (a fault the service meets is the
//! machine's `Fault` input) and steps the machine. The parties beside the
//! port are a [`Recorder`]: no data plane runs here, so the data stops at
//! once; the controller ends its tokens when told; the network stops when
//! told.
//!
//! - From a run with a received unit held and a unit lent, a restart ends
//!   reinstalled with nothing out of the port, the controller holds the next
//!   installation's permissions and a run on them starts
//!   ([`Case::Restart`]); a release ends released ([`Case::Release`]).
//!
//! A case reads `outstanding()` only of a port that holds an installation:
//! after a release it holds none, and the proof that nothing was out is the
//! retirement the port granted before it.
//! - After a fault the recovery takes back, a restart and a release end as
//!   they do from a run ([`Case::FaultTakenBack`]).
//! - A retirement refused for a unit held says what is out and ends once
//!   the unit comes home ([`Case::RetireHeld`]).
//! - A stop that fails once is asked again at the retry time and the end
//!   is reached ([`Case::StopFailsOnce`]).
//! - A deadline ends the request with what is waited for, and the next
//!   request goes on from there ([`Case::Deadline`]).
//! - After a group a failed replacement retained, a release's cleanup hands
//!   its sink the removal's terminal, once, and the release ends
//!   ([`Case::CleanupTerminal`]).

extern crate alloc;

use alloc::vec::Vec;
use core::mem::MaybeUninit;
use core::task::Context;

use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{DataPort, RxPull, TxFacts};
use oer_protocols_ieee80211_core_installation::{
    DataStop, End, Ended, Input, Network, Order, Permit, Request, Rest, Tell, Unreached, Waited,
    Waiting,
};
use oer_protocols_ieee80211_runtime::installation::{
    ChannelOf, MachineOf, Parties, PortParts, drive,
};
use oer_protocols_ieee80211_testing_conformance::FaultReach;
use oer_protocols_ieee80211_testing_conformance::data_port::{DataPortHarness, PROGRESS_STEPS};

/// The retry interval of the cases' machines: chosen, for the answers that
/// name nothing that wakes (a stop that failed, a cleanup that said later).
pub const RETRY: Duration = Duration::from_millis(1);

/// How long a case gives a request before its deadline, when it sets one.
const DEADLINE: Duration = Duration::from_millis(50);

/// A port's conformance harness, with what `drive` takes beside it.
pub trait InstallationHarness<'id>: DataPortHarness<'id> {
    /// The port, the installations a reinstall takes the next one from,
    /// and the sink, borrowed together for one step.
    fn drive_parts(
        &mut self,
    ) -> (
        &mut Self::Port,
        &mut <Self::Port as DataPort<'id>>::Installations,
        &mut Self::Sink,
    );
    /// Hold the next installation's permissions, as its controller does.
    fn adopt(&mut self, installed: <Self::Port as DataPort<'id>>::Installed);
}

/// What one request reached, as the controller hears it.
#[derive(Debug, Eq, PartialEq)]
pub enum Heard<F> {
    /// It reached what it asked.
    Reached(End),
    /// It did not.
    Unreached(Unreached<F>),
    /// The port is released.
    Released,
}

/// The parties of a case: what the machine told, ended and said.
pub struct Recorder<'id, P: DataPort<'id>> {
    orders: Vec<Order<'id>>,
    network: usize,
    installed: Vec<P::Installed>,
    heard: Vec<Heard<P::Fault>>,
    permits: Vec<Permit<'id>>,
    interrupted: usize,
    held: Vec<Waited>,
}

impl<'id, P: DataPort<'id>> Recorder<'id, P> {
    const fn new() -> Self {
        Self {
            orders: Vec::new(),
            network: 0,
            installed: Vec::new(),
            heard: Vec::new(),
            permits: Vec::new(),
            interrupted: 0,
            held: Vec::new(),
        }
    }

    fn reached(&self, end: End) -> usize {
        self.heard
            .iter()
            .filter(|heard| matches!(heard, Heard::Reached(reached) if *reached == end))
            .count()
    }

    fn released(&self) -> bool {
        self.heard
            .iter()
            .any(|heard| matches!(heard, Heard::Released))
    }

    fn deadlines(&self) -> usize {
        self.heard
            .iter()
            .filter(|heard| matches!(heard, Heard::Unreached(Unreached::Deadline(_))))
            .count()
    }
}

impl<'id, P: DataPort<'id>> Parties<'id, P> for Recorder<'id, P> {
    fn stop_data(&mut self) -> DataStop {
        // No data plane runs in the cases: the data is home at once.
        DataStop::Stopped
    }
    fn tell(&mut self, tell: Tell<'id>) {
        match tell {
            Tell::EndTokens(order) => self.orders.push(order),
            Tell::StopNetwork => self.network += 1,
            Tell::DataFlows => {}
        }
    }
    fn reinstalled(&mut self, ended: Ended<'id, P::Fault>, installed: P::Installed) {
        self.installed.push(installed);
        self.ended(ended);
    }
    fn ended(&mut self, ended: Ended<'id, P::Fault>) {
        match ended {
            Ended::Request(result, permit) => {
                self.permits.push(permit);
                self.heard.push(match result {
                    Ok(end) => Heard::Reached(end),
                    Err(unreached) => Heard::Unreached(unreached),
                });
            }
            Ended::Released => self.heard.push(Heard::Released),
        }
    }
    fn interrupted(&mut self) {
        self.interrupted += 1;
    }
    fn held(&mut self, waiting: &Waiting<P::Fault>) {
        self.held.push(waiting.class());
    }
}

/// One case of the suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// From a run with traffic, a restart ends reinstalled, nothing out.
    Restart,
    /// From a run with traffic, a release ends released, nothing out.
    Release,
    /// After a fault the recovery takes back, a restart and a release end
    /// as they do from a run.
    FaultTakenBack,
    /// A retirement refused for a unit held says what is out and ends once
    /// the unit comes home.
    RetireHeld,
    /// A stop that fails once is asked again at the retry time.
    StopFailsOnce,
    /// A deadline ends the request, and the next request goes on.
    Deadline,
    /// After a group a failed replacement retained, a release's cleanup
    /// hands its sink the removal's terminal, once, and the release ends.
    CleanupTerminal,
}

impl Case {
    /// Every case, in suite order.
    pub const ALL: [Self; 7] = [
        Self::Restart,
        Self::Release,
        Self::FaultTakenBack,
        Self::RetireHeld,
        Self::StopFailsOnce,
        Self::Deadline,
        Self::CleanupTerminal,
    ];
}

/// What a case found wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The machine did not reach this within the case's steps.
    Progress(&'static str),
    /// The port has these classes out where the case expects none.
    Out(u32),
    /// A status did not name what the case holds.
    Held,
    /// The fixture could not set up the case.
    Fixture,
}

/// Run `case` on a fresh harness, under a machine placed on the harness's
/// first installation.
///
/// # Errors
///
/// What the case found wrong.
pub fn check<'id, H: InstallationHarness<'id>>(
    harness: &mut H,
    case: Case,
) -> Result<(), Violation> {
    let stopped = harness.take_stopped().ok_or(Violation::Fixture)?;
    let right = harness.take_right().ok_or(Violation::Fixture)?;
    let mut place = MaybeUninit::uninit();
    let (machine, live) = MachineOf::<'id, H::Port>::place(&mut place, stopped, right, RETRY);
    let mut run = Run {
        machine,
        harness,
        recorder: Recorder::new(),
        rest: Rest::Round,
        woken: 0,
        adopted: 0,
        radio: None,
    };
    let channel = run.harness.channel();
    run.step(Input::Request(Request::run(live, channel, None)));
    run.until("started", |heard| heard.reached(End::Started) == 1)?;
    match case {
        Case::Restart => {
            let held = run.traffic()?;
            run.ask(|permit| Ok(Request::restart(permit, None)))?;
            run.home(held)?;
            run.until("reinstalled", |heard| heard.reached(End::Reinstalled) == 1)?;
            run.nothing_out()?;
            // The controller holds the next installation's permissions,
            // from the one call that ended the restart, and a run on them
            // starts.
            if run.adopted != 1 {
                return Err(Violation::Fixture);
            }
            let channel = run.harness.channel();
            run.ask(|permit| match permit {
                Permit::Live(live) => Ok(Request::run(live, channel, None)),
                Permit::Spent(_) => Err(Violation::Fixture),
            })?;
            run.until("started again", |heard| heard.reached(End::Started) == 2)
        }
        Case::Release => {
            let held = run.traffic()?;
            run.ask(|permit| Ok(Request::release(permit, Network::Stops, None)))?;
            run.home(held)?;
            run.until("released", Recorder::released)
        }
        Case::FaultTakenBack => {
            run.harness.fault(FaultReach::Whole);
            run.until("interrupted", |heard| heard.interrupted == 1)?;
            run.ask(|permit| Ok(Request::restart(permit, None)))?;
            run.until("reinstalled", |heard| heard.reached(End::Reinstalled) == 1)?;
            run.nothing_out()?;
            run.ask(|permit| Ok(Request::release(permit, Network::Stays, None)))?;
            run.until("released", Recorder::released)
        }
        Case::RetireHeld => {
            let held = run.traffic()?;
            run.ask(|permit| Ok(Request::release(permit, Network::Stays, None)))?;
            run.until("held for what is out", |heard| {
                heard.held.contains(&Waited::Outstanding)
            })?;
            if run.recorder.released() {
                return Err(Violation::Held);
            }
            run.home(held)?;
            run.until("released", Recorder::released)
        }
        Case::StopFailsOnce => {
            run.harness.fail_next_stop();
            run.ask(|permit| match permit {
                Permit::Live(live) => Ok(Request::halt(live, None)),
                Permit::Spent(_) => Err(Violation::Fixture),
            })?;
            run.until("halted", |heard| heard.reached(End::Halted) == 1)?;
            // The stop that failed names nothing that wakes: its step said
            // what it waits for and named the retry time.
            match run.radio {
                Some((Rest::Wake(Some(at)), now)) if now.checked_add(RETRY) == Some(at) => Ok(()),
                Some(_) | None => Err(Violation::Held),
            }
        }
        Case::CleanupTerminal => {
            run.harness.retain_group().map_err(|_| Violation::Fixture)?;
            // The command's fault ends the run; the recovery takes it back.
            run.until("interrupted", |heard| heard.interrupted == 1)?;
            let before = run.harness.sink_terminals();
            run.ask(|permit| Ok(Request::release(permit, Network::Stays, None)))?;
            run.until("released", Recorder::released)?;
            // The cleanup handed the sink the removal's terminal, once.
            let after = run.harness.sink_terminals();
            if after.removals == before.removals + 1 {
                Ok(())
            } else {
                Err(Violation::Held)
            }
        }
        Case::Deadline => {
            let held = run.traffic()?;
            let deadline = run
                .harness
                .now()
                .checked_add(DEADLINE)
                .ok_or(Violation::Fixture)?;
            run.ask(|permit| Ok(Request::release(permit, Network::Stays, Some(deadline))))?;
            run.until("the deadline", |heard| heard.deadlines() == 1)?;
            if run.recorder.released() {
                return Err(Violation::Held);
            }
            // The next request goes on from where the machine is.
            run.ask(|permit| Ok(Request::release(permit, Network::Stays, None)))?;
            run.home(held)?;
            run.until("released", Recorder::released)
        }
    }
}

/// What a case holds out of the port: a received unit, and a unit lent.
struct Held<'id, P: DataPort<'id>> {
    received: oer_protocols_ieee80211_contracts_port::PortRxBuffer<'id, P>,
    lent: <P::Types as oer_protocols_ieee80211_contracts_port::PortTypes<'id>>::LentRx,
}

/// The machine of a case, its parties and its harness.
struct Run<'m, 'id, H: InstallationHarness<'id>> {
    machine: &'m mut MachineOf<'id, H::Port>,
    harness: &'m mut H,
    recorder: Recorder<'id, H::Port>,
    /// The last step's rest.
    rest: Rest,
    /// The wakes of the port's waker at the last step.
    woken: usize,
    /// The next installations' permissions the harness adopted.
    adopted: usize,
    /// The rest, and the instant, of the step that first said the radio's
    /// stop is unconfirmed.
    radio: Option<(Rest, oer_portable_time::Instant)>,
}

impl<'id, H: InstallationHarness<'id>> Run<'_, 'id, H> {
    /// One step from `input`, then the inputs its messages call for: the
    /// controller's tokens' end and the network's stop. The last rest.
    fn step(&mut self, input: Input<'id, ChannelOf<'id, H::Port>>) -> Rest {
        let now = self.harness.now();
        let mut rest = self.drive(input);
        loop {
            if let Some(order) = self.recorder.orders.pop() {
                self.harness.end_tokens();
                rest = self.drive(Input::TokensEnded(order.ended()));
            } else if self.recorder.network > 0 {
                self.recorder.network -= 1;
                rest = self.drive(Input::NetworkStopped);
            } else {
                break;
            }
        }
        for installed in self.recorder.installed.drain(..) {
            self.harness.adopt(installed);
            self.adopted += 1;
        }
        if self.recorder.held.last() == Some(&Waited::Radio) && self.radio.is_none() {
            self.radio = Some((rest, now));
        }
        self.rest = rest;
        self.woken = self.harness.wakes();
        rest
    }

    fn drive(&mut self, input: Input<'id, ChannelOf<'id, H::Port>>) -> Rest {
        let waker = self.harness.waker();
        let now = self.harness.now();
        let (port, installations, sink) = self.harness.drive_parts();
        drive(
            self.machine,
            input,
            &mut PortParts {
                port,
                installations,
                sink,
            },
            &mut self.recorder,
            &mut Context::from_waker(&waker),
            now,
        )
    }

    /// Ask from the permit the last end returned.
    fn ask(
        &mut self,
        request: impl FnOnce(Permit<'id>) -> Result<Request<'id, ChannelOf<'id, H::Port>>, Violation>,
    ) -> Result<(), Violation> {
        let permit = self.recorder.permits.pop().ok_or(Violation::Fixture)?;
        let request = request(permit)?;
        self.step(Input::Request(request));
        Ok(())
    }

    /// Serve the port once, as a data plane does: a fault the service meets
    /// is the machine's input, and an owner come home (a unit returned or
    /// recycled, which the port's facts and its lend's return say) is a
    /// wake. What the service answers, and whether it woke.
    fn serve(&mut self) -> (Option<Rest>, bool) {
        let waker = self.harness.waker();
        let now = self.harness.now();
        let (port, sink) = self.harness.parts();
        let mut cx = Context::from_waker(&waker);
        let facts = port.poll_facts(&mut cx);
        let returned = port.poll_lent_returned(&mut cx).is_ready();
        let came_home = facts.rx.is_raised() || returned;
        let faulted = port
            .service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink)
            .is_err();
        (faulted.then(|| self.step(Input::Fault)), came_home)
    }

    /// Turn until `done` holds of the recorder, only as the rests name: the
    /// next round, the instant a rest names (the fixture's time is advanced
    /// to it), or a wake of the waker the port was lent.
    fn until(
        &mut self,
        what: &'static str,
        done: impl Fn(&Recorder<'id, H::Port>) -> bool,
    ) -> Result<(), Violation> {
        let mut rest = self.rest;
        for _ in 0..PROGRESS_STEPS {
            if done(&self.recorder) {
                return Ok(());
            }
            self.harness.settle();
            let (faulted, came_home) = self.serve();
            if let Some(faulted) = faulted {
                rest = faulted;
            }
            let woken = came_home || self.harness.wakes() != self.woken;
            // The port names the earliest instant a call must run without a
            // fact (`DataPort::deadline`): a call is due then as at a time a
            // rest names.
            let due = {
                let (port, _) = self.harness.parts();
                port.deadline()
            };
            let at = match (rest, due) {
                (Rest::Round, _) => None,
                (Rest::Wake(Some(at)), Some(due)) => Some(at.min(due)),
                (Rest::Wake(at), None) | (Rest::Wake(None), at) => at,
            };
            let turn = match (rest, at) {
                (Rest::Round, _) => true,
                (Rest::Wake(_), Some(at)) => {
                    if !woken && let Some(by) = at.checked_duration_since(self.harness.now()) {
                        self.harness.advance(by);
                    }
                    true
                }
                (Rest::Wake(_), None) => woken,
            };
            if turn {
                rest = self.step(Input::Turn);
            }
        }
        if done(&self.recorder) {
            Ok(())
        } else {
            Err(Violation::Progress(what))
        }
    }

    /// Traffic of a run: two received units, one held and one lent.
    fn traffic(&mut self) -> Result<Held<'id, H::Port>, Violation> {
        self.harness.deliver(2);
        let mut units = Vec::new();
        for _ in 0..PROGRESS_STEPS {
            let now = self.harness.now();
            let (port, _) = self.harness.parts();
            match port.receive(1, now) {
                Ok(RxPull::Unit { frame, .. }) => {
                    units.push(frame);
                    if units.len() == 2 {
                        break;
                    }
                }
                Ok(RxPull::Empty | RxPull::Later(_)) | Err(_) => self.harness.settle(),
            }
        }
        let lend = units.pop().ok_or(Violation::Fixture)?;
        let received = units.pop().ok_or(Violation::Fixture)?;
        let (port, _) = self.harness.parts();
        let lent = port.lend(lend).map_err(|_| Violation::Fixture)?;
        Ok(Held { received, lent })
    }

    /// What the case held comes home.
    fn home(&mut self, held: Held<'id, H::Port>) -> Result<(), Violation> {
        let Held { received, lent } = held;
        drop(lent);
        let (port, _) = self.harness.parts();
        port.recycle(received).map_err(|_| Violation::Fixture)
    }

    /// Nothing out of the port.
    fn nothing_out(&mut self) -> Result<(), Violation> {
        let (port, _) = self.harness.parts();
        let out = port.outstanding();
        if out.is_empty() {
            Ok(())
        } else {
            Err(Violation::Out(out.classes()))
        }
    }
}
