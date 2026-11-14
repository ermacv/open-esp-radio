//! The machine's tests: the model's search, walked over the Rust machine,
//! and the table of its transitions against the model's.
//!
//! The search is `explore` of `design/installation-machine-model.py` (the
//! notes), event for event: the environment answers every ask with every
//! answer the port contract allows, sends every request in every state and
//! a fault in every phase, within the same bounds, and checks the model's
//! five properties. Its table of transitions is compared with
//! `tests/model-table.txt`, the model's own (`--table`).

extern crate std;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

use oer_protocols_ieee80211_contracts_port::{
    ArenaState, Outstanding, ReinstallRefused, Retired, RunPhase, StartFailure, StateSerial,
    StopCause, StopUnconfirmed, Stopped,
};

use super::*;

/// The channel of the search's requests.
type Channel = u8;
/// The search's fault.
type Fault = ();

/// A machine's state, without its proof: what the search revisits.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Key {
    phase: Phase,
    goal: Option<Goal<Channel>>,
    data: Data,
    net: Net,
    ended: bool,
    waits: Waits,
    flight: Flight,
}

impl<'id, F> Machine<'id, Channel, F> {
    pub(crate) fn key(&self) -> Key {
        Key {
            phase: self.phase(),
            goal: self.goal,
            data: self.data,
            net: self.net,
            ended: self.ended,
            waits: self.waits,
            flight: self.flight,
        }
    }

    /// The machine in the state `key` names, its phase's proof issued.
    pub(crate) fn of_key(key: Key) -> Self {
        let state = match key.phase {
            Phase::Stopped => State::Stopped(Stopped::issue(StateSerial::FIRST)),
            Phase::Starting => State::Starting,
            Phase::Running => State::Running,
            Phase::Stopping => State::Stopping,
            Phase::Settling => State::Settling(Stopped::issue(StateSerial::FIRST)),
            Phase::Retiring => State::Retiring,
            Phase::Retired => State::Retired(Retired::issue(StateSerial::FIRST)),
            Phase::Reinstalling => State::Reinstalling,
            Phase::Releasing => State::Releasing,
            Phase::Released => State::Released,
        };
        Self {
            state,
            goal: key.goal,
            data: key.data,
            net: key.net,
            ended: key.ended,
            waits: key.waits,
            flight: key.flight,
            fault: PhantomData,
        }
    }
}

/// The proof the model's state holds: `has`.
fn has(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Stopped | Phase::Settling => Some("stopped"),
        Phase::Retired => Some("retired"),
        _ => None,
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Stopped => "stopped",
        Phase::Starting => "starting",
        Phase::Running => "running",
        Phase::Stopping => "stopping",
        Phase::Settling => "settling",
        Phase::Retiring => "retiring",
        Phase::Retired => "retired",
        Phase::Reinstalling => "reinstalling",
        Phase::Releasing => "releasing",
        Phase::Released => "released",
    }
}

fn goal_name(goal: Option<Goal<Channel>>) -> Option<&'static str> {
    goal.map(|goal| match goal {
        Goal::Run(_) => "run",
        Goal::Halt => "halt",
        Goal::Restart { .. } => "restart",
        Goal::Release(Network::Stays) => "release_stays",
        Goal::Release(Network::Stops) => "release_stops",
    })
}

/// What one step wrote: the asks and the reports, by the model's names.
#[derive(Default)]
struct Recorder {
    asks: Vec<&'static str>,
    reports: Vec<&'static str>,
}

impl<'id> Outbox<'id, Channel, Fault> for Recorder {
    fn start(&mut self, _: Asked<Start>, _: Stopped<'id>, _: Channel) {
        self.asks.push("start");
    }
    fn cancel_start(&mut self) -> Asked<Start> {
        Asked::new()
    }
    fn stop(&mut self, _: Asked<Stop>) {
        self.asks.push("stop");
    }
    fn stop_data(&mut self, _: Asked<StopData>) {
        self.asks.push("stop_data");
    }
    fn end_tokens(&mut self, _: Asked<EndTokens>) {
        self.asks.push("end_tokens");
    }
    fn drain(&mut self, _: Asked<Drain>, _: &Stopped<'id>) {
        self.asks.push("drain");
    }
    fn stop_network(&mut self, _: Asked<StopNetwork>) {
        self.asks.push("stop_net");
    }
    fn retire(&mut self, _: Asked<Retire>, _: Stopped<'id>) {
        self.asks.push("retire");
    }
    fn reinstall(&mut self, _: Asked<Reinstall>, _: Retired<'id>, _: u32) {
        self.asks.push("reinstall");
    }
    fn release(&mut self, _: Asked<Release>, _: Retired<'id>) {
        self.asks.push("release");
    }
    fn refused(&mut self, _: Request<Channel>, _: Phase) {
        self.reports.push("refused");
    }
    fn ended(&mut self, end: Result<End, Unreached<'_, Channel, Fault>>) {
        self.reports.push(match end {
            Ok(End::Started) => "started",
            Ok(End::Halted) => "halted",
            Ok(End::Reinstalled) => "reinstalled",
            Ok(End::Released) => "released",
            Err(Unreached::StartRefused(_)) => "start_refused",
            Err(Unreached::RestartRefused(_)) => "restart_refused",
            Err(Unreached::Superseded(_)) => "superseded",
            Err(Unreached::Released) => "ended",
        });
    }
    fn status(&mut self, status: Status<'_, Fault>) {
        self.reports.push(match status {
            Status::Interrupted => "interrupted",
            Status::Held(_) => "held",
        });
    }
    fn deadline(&mut self, _: Option<Instant>) {}
}

const REQUESTS: [&str; 6] = [
    "start",
    "retune",
    "halt",
    "restart",
    "release_stays",
    "release_stops",
];

fn answers(ask: &str) -> &'static [&'static str] {
    match ask {
        "start" => &["start_ok", "start_refused", "start_unstopped", "stale"],
        "stop" => &["stop_ok", "stop_failed", "stop_released"],
        "stop_data" => &["data_stopped"],
        "end_tokens" => &["ready"],
        "drain" => &[
            "drained",
            "drain_refused",
            "drain_later",
            "drain_faulted",
            "stale",
        ],
        "stop_net" => &["net_stopped"],
        "retire" => &["retired", "retire_refused", "retire_refused_fault", "stale"],
        "reinstall" => &["reinstalled", "reinstall_refused", "stale"],
        "release" => &["released", "stale"],
        _ => &[],
    }
}

const REFUSALS: [&str; 11] = [
    "start_refused",
    "start_unstopped",
    "stop_failed",
    "drain_refused",
    "drain_later",
    "drain_faulted",
    "retire_refused",
    "reinstall_refused",
    "stale",
    "stop_released",
    "retire_refused_fault",
];

fn gives_proof(ask: &str) -> Option<&'static str> {
    match ask {
        "start" | "retire" => Some("stopped"),
        "reinstall" | "release" => Some("retired"),
        _ => None,
    }
}

fn borrows_proof(ask: &str) -> Option<&'static str> {
    (ask == "drain").then_some("stopped")
}

const FINAL: [&str; 8] = [
    "started",
    "halted",
    "reinstalled",
    "released",
    "start_refused",
    "restart_refused",
    "superseded",
    "ended",
];

/// What blocks a refused retirement, as the environment knows it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Blocks {
    Nothing,
    Fault,
    Returns,
}

/// Why the last drain was refused, as the environment knows it: a sink
/// full, a cleanup to ask later, a fault.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Refused {
    No,
    Full,
    Later,
    Fault,
}

/// What the environment knows of the current installation: its tokens
/// ended, its unconsumed items taken, why its last drain was refused, and
/// what blocks its retirement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Inst {
    tokens: bool,
    items: bool,
    refused: Refused,
    blocks: Blocks,
}

const FRESH: Inst = Inst {
    tokens: false,
    items: false,
    refused: Refused::No,
    blocks: Blocks::Nothing,
};

/// The budget: requests, faults, late stops and deadlines, refusals.
type Budget = (u8, u8, u8, u8);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Env {
    s: Key,
    pending: BTreeSet<&'static str>,
    budget: Budget,
    open: Option<&'static str>,
    inst: Inst,
}

fn at_rest(env: &Env) -> bool {
    matches!(
        env.s.phase,
        Phase::Stopped | Phase::Running | Phase::Released | Phase::Retired
    ) && env.s.goal.is_none()
        && env.pending.is_empty()
}

/// Every event the environment can send in `env`: the event, the ask it
/// answers, the budget after it.
fn environment(env: &Env) -> Vec<(String, Option<&'static str>, Budget)> {
    let (nreq, nfault, nlate, nref) = env.budget;
    let s = env.s;
    let mut events = Vec::new();
    for ask in &env.pending {
        for answer in answers(ask) {
            let refuse = REFUSALS.contains(answer);
            if refuse && nref == 0 {
                continue;
            }
            events.push((
                answer.to_string(),
                Some(*ask),
                (nreq, nfault, nlate, nref - u8::from(refuse)),
            ));
        }
    }
    if env.pending.contains("stop") && nlate > 0 {
        events.push(("stop_late".into(), None, (nreq, nfault, nlate - 1, nref)));
    }
    if nreq > 0 {
        for request in REQUESTS {
            events.push((
                format!("req_{request}"),
                None,
                (nreq - 1, nfault, nlate, nref),
            ));
        }
    }
    if nfault > 0 && s.phase != Phase::Released {
        events.push((
            "fault_recovered".into(),
            None,
            (nreq, nfault - 1, nlate, nref),
        ));
        events.push(("fault_kept".into(), None, (nreq, nfault - 1, nlate, nref)));
    }
    let drain_owed =
        has(s.phase) == Some("stopped") && s.waits.drained && !env.pending.contains("drain");
    if (s.phase == Phase::Stopping && !env.pending.contains("stop"))
        || (env.inst.refused == Refused::Later && drain_owed)
    {
        events.push(("again".into(), None, env.budget));
    }
    if env.inst.refused == Refused::Full && drain_owed {
        events.push(("room".into(), None, env.budget));
    }
    if env.inst.refused == Refused::Fault && drain_owed {
        events.push(("fault_recovered".into(), None, env.budget));
    }
    if s.phase == Phase::Settling && s.waits.returns {
        match env.inst.blocks {
            Blocks::Fault => events.push(("fault_recovered".into(), None, env.budget)),
            Blocks::Returns => events.push(("returned".into(), None, env.budget)),
            Blocks::Nothing => {}
        }
    }
    if nlate > 0 && !matches!(goal_name(s.goal), None | Some("run")) {
        events.push(("deadline".into(), None, (nreq, nfault, nlate - 1, nref)));
    }
    if nlate > 0 && s.phase != Phase::Released {
        // A wake says only "look again": these may come when nothing is
        // owed.
        for look in ["room", "returned", "again"] {
            events.push((look.into(), None, (nreq, nfault, nlate - 1, nref)));
        }
    }
    events
}

const fn stop(cause: StopCause<Fault>) -> StopUnconfirmed<Fault> {
    StopUnconfirmed {
        cause,
        outstanding: OUT,
    }
}

const OUT: Outstanding<Fault> = Outstanding {
    phase: RunPhase::Stopped,
    fault: None,
    received: 1,
    lent: 0,
    removals: 0,
    retained_groups: 0,
    unconsumed: 0,
    transmit_out: 0,
    queued: [0; oer_protocols_ieee80211_contracts_port::ACCESS_CATEGORIES],
    running: 0,
    arenas: ArenaState::Returnable,
};

const OUT_FAULT: Outstanding<Fault> = Outstanding {
    fault: Some(()),
    received: 0,
    ..OUT
};

static STOP_LATE: StopUnconfirmed<Fault> = stop(StopCause::Deadline);
static STOP_FAILED: StopUnconfirmed<Fault> = stop(StopCause::Device(()));
static START_UNSTOPPED: StopUnconfirmed<Fault> = stop(StopCause::CoreEnabled);
static OUT_HELD: Outstanding<Fault> = OUT;
static OUT_HELD_FAULT: Outstanding<Fault> = OUT_FAULT;
static FAULT: Fault = ();

/// The stop's ticket a stop that goes on borrows.
static STOP_TICKET: Asked<Stop> = Asked::new();

/// The machine's event for the model's `name`, answering `ask`.
fn event(name: &str, ask: Option<&str>) -> Event<'static, 'static, Channel, Fault> {
    let stopped = || Stopped::issue(StateSerial::FIRST);
    let retired = || Retired::issue(StateSerial::FIRST);
    let request = |request| Event::Request {
        request,
        deadline: None,
    };
    match (name, ask) {
        ("req_start", _) => request(Request::Start(6)),
        ("req_retune", _) => request(Request::Retune(11)),
        ("req_halt", _) => request(Request::Halt),
        ("req_restart", _) => request(Request::Restart { epoch: 2 }),
        ("req_release_stays", _) => request(Request::Release(Network::Stays)),
        ("req_release_stops", _) => request(Request::Release(Network::Stops)),
        ("start_ok", _) => Event::Start(Asked::new(), StartAnswer::Started),
        ("start_refused", _) => Event::Start(
            Asked::new(),
            StartAnswer::Refused(stopped(), StartFailure::Device),
        ),
        ("start_unstopped", _) => Event::Start(
            Asked::new(),
            StartAnswer::Unstopped {
                cause: StartFailure::Device,
                stop: &START_UNSTOPPED,
            },
        ),
        ("stale", Some("start")) => Event::Start(Asked::new(), StartAnswer::Stale),
        ("stale", Some("drain")) => Event::Drain(Asked::new(), DrainAnswer::Stale),
        ("stale", Some("retire")) => Event::Retire(Asked::new(), RetireAnswer::Stale),
        ("stale", Some("reinstall")) => Event::Reinstall(Asked::new(), ReinstallAnswer::Stale),
        ("stale", Some("release")) => Event::Release(Asked::new(), ReleaseAnswer::Stale),
        ("stop_ok", _) => Event::Stop(Asked::new(), StopAnswer::Stopped(stopped())),
        ("stop_failed", _) => Event::Stop(Asked::new(), StopAnswer::Failed(&STOP_FAILED)),
        ("stop_released", _) => Event::Stop(Asked::new(), StopAnswer::Released),
        ("stop_late", _) => Event::StopGoesOn(&STOP_TICKET, &STOP_LATE),
        ("data_stopped", _) => Event::DataStopped(Asked::new()),
        ("ready", _) => Event::Ready(Asked::new()),
        ("drained", _) => Event::Drain(Asked::new(), DrainAnswer::Drained),
        ("drain_refused", _) => Event::Drain(Asked::new(), DrainAnswer::Refused),
        ("drain_later", _) => Event::Drain(Asked::new(), DrainAnswer::Later),
        ("drain_faulted", _) => Event::Drain(Asked::new(), DrainAnswer::Faulted(&FAULT)),
        ("net_stopped", _) => Event::NetworkStopped(Asked::new()),
        ("retired", _) => Event::Retire(Asked::new(), RetireAnswer::Retired(retired())),
        ("retire_refused", _) => {
            Event::Retire(Asked::new(), RetireAnswer::Refused(stopped(), &OUT_HELD))
        }
        ("retire_refused_fault", _) => Event::Retire(
            Asked::new(),
            RetireAnswer::Refused(stopped(), &OUT_HELD_FAULT),
        ),
        ("reinstalled", _) => {
            Event::Reinstall(Asked::new(), ReinstallAnswer::Reinstalled(stopped()))
        }
        ("reinstall_refused", _) => Event::Reinstall(
            Asked::new(),
            ReinstallAnswer::Refused(retired(), ReinstallRefused::SameEpoch),
        ),
        ("released", _) => Event::Release(Asked::new(), ReleaseAnswer::Released),
        ("fault_recovered", _) => Event::Fault(FaultAnswer::TakenBack),
        ("fault_kept", _) => Event::Fault(FaultAnswer::Kept {
            fault: &FAULT,
            outstanding: &OUT_HELD_FAULT,
        }),
        ("returned", _) => Event::Returned,
        ("again", _) => Event::Again,
        ("room", _) => Event::Room,
        ("deadline", _) => Event::Deadline,
        (other, ask) => panic!("no event {other} answering {ask:?}"),
    }
}

/// One row of the table: the next phase, the asks, the reports.
type Out = (String, Vec<String>, Vec<String>);

pub(crate) struct Search {
    pub(crate) states: usize,
    pub(crate) machine_states: usize,
    pub(crate) problems: BTreeMap<String, usize>,
    pub(crate) dead: usize,
    table: BTreeMap<(String, String), BTreeSet<Out>>,
}

impl Search {
    /// The table, as the model prints it with `--table`.
    pub(crate) fn table(&self) -> String {
        let mut text = String::new();
        for ((phase, event), outs) in &self.table {
            for (to, asks, reports) in outs {
                let asks = if asks.is_empty() {
                    "—".to_string()
                } else {
                    asks.join(", ")
                };
                let reports = if reports.is_empty() {
                    "—".to_string()
                } else {
                    reports.join(", ")
                };
                text.push_str(&format!(
                    "| {phase} | {event} | {to} | {asks} | {reports} |\n"
                ));
            }
        }
        text
    }
}

/// The model's search over the Rust machine, within `budget`.
pub(crate) fn explore(
    budget: Budget,
    step: &dyn Fn(Key, &str, Option<&str>) -> (Key, Recorder),
) -> Search {
    let start = Env {
        s: Machine::<Channel, Fault>::place(
            &mut core::mem::MaybeUninit::uninit(),
            Stopped::issue(StateSerial::FIRST),
        )
        .key(),
        pending: BTreeSet::new(),
        budget,
        open: None,
        inst: FRESH,
    };
    let mut seen = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([start]);
    let mut edges: HashMap<Env, Vec<Env>> = HashMap::new();
    let mut problems = BTreeMap::<String, usize>::new();
    let mut problem = |kind: &str| *problems.entry(kind.to_string()).or_default() += 1;
    let mut table = BTreeMap::<(String, String), BTreeSet<Out>>::new();
    while let Some(env) = queue.pop_front() {
        let mut outs = Vec::new();
        for (name, answered, budget2) in environment(&env) {
            let s = env.s;
            let mut pending = env.pending.clone();
            if let Some(answered) = answered {
                pending.remove(answered);
            }
            let (ns, recorded) = step(s, &name, answered);
            let (asks, reports) = (recorded.asks, recorded.reports);
            let mut inst = env.inst;
            match name.as_str() {
                "ready" => inst.tokens = true,
                "drained" => {
                    inst.items = true;
                    inst.refused = Refused::No;
                }
                "drain_refused" => inst.refused = Refused::Full,
                "drain_later" => inst.refused = Refused::Later,
                "drain_faulted" => inst.refused = Refused::Fault,
                "retire_refused_fault" => inst.blocks = Blocks::Fault,
                "retire_refused" => inst.blocks = Blocks::Returns,
                "fault_recovered" | "returned" => inst.blocks = Blocks::Nothing,
                "reinstalled" => inst = FRESH,
                _ => {}
            }
            table
                .entry((phase_name(s.phase).to_string(), name.clone()))
                .or_default()
                .insert((
                    phase_name(ns.phase).to_string(),
                    asks.iter().map(|ask| ask.to_string()).collect(),
                    reports.iter().map(|report| report.to_string()).collect(),
                ));
            // The proofs, as the model counts them.
            let arriving = match name.as_str() {
                "start_refused"
                | "stop_ok"
                | "retire_refused"
                | "retire_refused_fault"
                | "reinstalled" => Some("stopped"),
                "retired" | "reinstall_refused" => Some("retired"),
                _ => None,
            };
            if arriving.is_some() && has(s.phase).is_some() {
                problem("two proofs");
            }
            let mut held = arriving.or(has(s.phase));
            if matches!(name.as_str(), "stale" | "stop_released") {
                held = None;
            }
            if name.starts_with("fault_") && s.phase == Phase::Starting {
                pending.remove("start");
            }
            for ask in &asks {
                let need = gives_proof(ask).or(borrows_proof(ask));
                if need.is_some() && held != need {
                    problem("ask without its proof");
                }
                if gives_proof(ask).is_some() {
                    held = None;
                }
                if *ask == "drain" {
                    inst.refused = Refused::No;
                }
                if *ask == "retire"
                    && !(ns.data == Data::Stopped && !ns.waits.any() && inst.tokens && inst.items)
                {
                    problem("retire asked early");
                }
                if *ask == "release"
                    && goal_name(ns.goal) == Some("release_stops")
                    && ns.net != Net::Stopped
                {
                    problem("release before the network stopped");
                }
                if *ask == "start"
                    && goal_name(ns.goal) == Some("run")
                    && s.phase == Phase::Stopped
                    && ns.data != Data::Stopped
                {
                    problem("start with data not stopped");
                }
                if *ask == "stop" {
                    pending.remove("start");
                } else if pending.contains(ask) {
                    problem("asked twice");
                }
                pending.insert(ask);
            }
            if held != has(ns.phase) {
                problem("proof not accounted");
            }
            // One end a request.
            let mut opened = env.open;
            let closes: Vec<&str> = reports
                .iter()
                .copied()
                .filter(|report| FINAL.contains(report))
                .collect();
            if let Some(request) = name.strip_prefix("req_") {
                if !reports.contains(&"refused") {
                    if opened.is_some() && closes.is_empty() {
                        problem("two requests open");
                    }
                    let superseded = closes.iter().filter(|c| **c == "superseded").count();
                    let own = closes.len() - superseded;
                    opened = if own > 0 {
                        None
                    } else {
                        REQUESTS.iter().copied().find(|r| *r == request)
                    };
                    if superseded > 0 && env.open.is_none() {
                        problem("superseded nothing");
                    }
                }
            } else if !closes.is_empty() {
                if opened.is_none() {
                    problem("an end with no request");
                }
                if closes.len() > 1 {
                    problem("two ends");
                }
                opened = None;
            }
            let next = Env {
                s: ns,
                pending,
                budget: budget2,
                open: opened,
                inst,
            };
            if at_rest(&next) && next.open.is_some() {
                problem("at rest with a request open");
            }
            outs.push(next.clone());
            if seen.insert(next.clone()) {
                queue.push_back(next);
            }
        }
        edges.insert(env, outs);
    }
    // No dead end: from every state a state at rest is reachable.
    let mut back: HashMap<&Env, Vec<&Env>> = HashMap::new();
    for (from, outs) in &edges {
        for to in outs {
            back.entry(to).or_default().push(from);
        }
    }
    let mut good: HashSet<&Env> = seen.iter().filter(|env| at_rest(env)).collect();
    let mut queue: VecDeque<&Env> = good.iter().copied().collect();
    while let Some(to) = queue.pop_front() {
        for from in back.get(to).into_iter().flatten() {
            if good.insert(from) {
                queue.push_back(from);
            }
        }
    }
    Search {
        states: seen.len(),
        machine_states: seen.iter().map(|env| env.s).collect::<HashSet<_>>().len(),
        problems,
        dead: seen.len() - good.len(),
        table,
    }
}

/// One step of the Rust machine from the state `key`.
fn machine_step(key: Key, name: &str, ask: Option<&str>) -> (Key, Recorder) {
    let mut machine = Machine::<Channel, Fault>::of_key(key);
    let mut recorder = Recorder::default();
    machine.step(event(name, ask), &mut recorder);
    (machine.key(), recorder)
}

/// The model's bounds: requests, faults, late stops and deadlines,
/// refusals.
const BUDGET: Budget = (3, 1, 1, 2);

#[test]
fn the_search_finds_every_property_of_the_model_held() {
    let search = explore(BUDGET, &machine_step);
    assert!(search.problems.is_empty(), "{:?}", search.problems);
    assert_eq!(search.dead, 0, "dead ends");
    // The model's own counts at these bounds.
    assert_eq!((search.states, search.machine_states), (4190, 174));
}

#[test]
fn the_search_holds_at_the_wider_bounds_too() {
    let search = explore((4, 2, 2, 3), &machine_step);
    assert!(search.problems.is_empty(), "{:?}", search.problems);
    assert_eq!(search.dead, 0, "dead ends");
}

/// The table the model prints with `--table`, committed:
/// `python3 -I design/installation-machine-model.py --table | grep '^|'`
/// in the notes refreshes it.
const MODEL_TABLE: &str = include_str!("../tests/model-table.txt");

#[test]
fn every_transition_is_the_models() {
    let search = explore(BUDGET, &machine_step);
    let rust = search.table();
    for (rust, model) in rust.lines().zip(MODEL_TABLE.lines()) {
        assert_eq!(rust, model);
    }
    assert_eq!(rust.lines().count(), MODEL_TABLE.lines().count());
    assert_eq!(rust.lines().count(), 200);
}
