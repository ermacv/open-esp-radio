//! The HIL radio pair: two ESP32-S31 boards, the receiver
//! (`esp32s31-rx-ring`) and the sender (`esp32s31-case-c-sender`). The host
//! tells each its peer's station address, which the other's bring-up
//! answered. Every step is a request the device answers; the cases and the
//! verdict are this program's.
//!
//! Each boot's snapshots (before and after the bring-up) and the receive
//! counters at each window's end are kept as measurements, so that
//! `radio-pair-compare` prints the words a deaf boot and a hearing boot of
//! the same image differ in.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

use std::{
    process::ExitCode,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use oer_hil_services_radio_pair::{
    self as service, BringUp, BroughtUp, Counters, Expect, Expecting, HandOver, HandedOver,
    ReceiveCounters, ReceiverRequest, Record, Round, SenderRequest, Sent, SnapshotCold,
    SnapshotPage, SnapshotUp, Stall, Stood, Stop, Stopped, Target, Targeted, Turn, Turned, Window,
    Windowed,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{Case, Cx, Failure, Role, Scenario, check};

const RECEIVER: &str = "receiver";
const SENDER: &str = "sender";

const ROLES: &[Role] = &[
    Role::new(RECEIVER, "esp32s31-rx-ring", &[ReceiverRequest::KEYS]),
    Role::new(SENDER, "esp32s31-case-c-sender", &[SenderRequest::KEYS]),
];

const SCENARIO: Scenario = Scenario::with_roles("radio-pair", ROLES).using_air_exclusively();

const CASES: [Case; 7] = [
    Case {
        name: "cold-snapshots",
        run: cold_snapshots,
    },
    Case {
        name: "bring-up",
        run: bring_up,
    },
    Case {
        name: "peer",
        run: peer,
    },
    // The radio up and the sender silent: the hand-over alone.
    Case {
        name: "hand-over",
        run: hand_over,
    },
    Case {
        name: "case-c",
        run: case_c,
    },
    Case {
        name: "stop",
        run: stop,
    },
    // Last: it boots both boards afresh.
    Case {
        name: "first-frame",
        run: first_frame::first_frame,
    },
];

/// One step that answers at once.
const STEP: Duration = Duration::from_secs(5);
/// The bring-up: the PHY's registration and calibration, the MAC's cold
/// start, the channel.
const BRING_UP: Duration = Duration::from_secs(30);
/// Case C's rounds, and how long they may take: about 45 s, past R4's end
/// (about 30 s); R1 to R4 take their counts inside their own windows.
const ROUNDS: u32 = 6_000;
const ROUNDS_TIMEOUT: Duration = Duration::from_secs(300);
/// The receiver's windows (`tasks/277`, `281`).
const WINDOW_MS: u32 = 2_000;
const STALL_MS: u32 = 100;
const EXIT_MS: u32 = 1_000;
const TURNS: u32 = 50;
const TURN_MS: u32 = 200;
/// The pass thresholds of `tasks/277` and `281`.
const R1_UNITS: u32 = 1_000;
const R2_CHAINS: u32 = 200;
const R3_UNITS: u32 = 100;
const R4_UNITS: u32 = 50;
/// The receiver's spares and buffers (`esp32s31-rx-ring`).
const SPARES: u8 = 16;
const BUFFERS: u8 = 32;
/// Case C's long cases: 700 and 1 500 octets (`case_c::Case::tag`).
const LONG_TAGS: [u32; 2] = [5, 6];

/// The addresses the bring-up gave, which the later cases use.
#[derive(Clone, Copy)]
struct Addresses {
    receiver: [u8; 6],
    sender: [u8; 6],
}

static ADDRESSES: Mutex<Option<Addresses>> = Mutex::new(None);

mod first_frame;

fn main() -> ExitCode {
    oer_tools_hil_harness::main(&SCENARIO, &CASES)
}

/// Which snapshot of a boot.
#[derive(Clone, Copy)]
enum Snapshot {
    /// Before the bring-up.
    Cold,
    /// After it.
    Up,
}

/// `role`'s whole `snapshot`, page by page.
fn snapshot(
    cx: &mut Cx<'_>,
    role: &str,
    snapshot: Snapshot,
) -> Result<Vec<service::Reading>, Failure> {
    let device = cx.device(role)?;
    let mut readings = Vec::new();
    let mut next = 0_u8;
    loop {
        let answer = match snapshot {
            Snapshot::Cold => device.call(SnapshotCold { page: next }, STEP)?,
            Snapshot::Up => device.call(SnapshotUp { page: next }, STEP)?,
        };
        match answer {
            SnapshotPage::Readings {
                page,
                pages,
                readings: these,
            } => {
                check(
                    page == next,
                    format!("{role}: page {next} answered as {page}"),
                )?;
                readings.extend(these);
                next = next.saturating_add(1);
                if next >= pages {
                    return Ok(readings);
                }
            }
            SnapshotPage::Refused(refusal) => {
                return Err(Failure::Fail(format!(
                    "{role}: snapshot page {next} refused: {refusal:?}"
                )));
            }
        }
    }
}

/// Keep `value` as the measurement `name` of `role`.
fn keep(cx: &mut Cx<'_>, role: &str, name: &str, value: &impl serde::Serialize) {
    let value = serde_json::to_value(value)
        .unwrap_or_else(|error| serde_json::Value::String(format!("not serialized: {error}")));
    cx.measure(format!("{role}/{name}"), value);
}

fn counters(cx: &mut Cx<'_>, role: &str, name: &str) -> Result<(), Failure> {
    let counted = cx.device(role)?.call(ReceiveCounters, STEP)?;
    if let Counters::Refused(refusal) = &counted {
        return Err(Failure::Fail(format!(
            "{role}: counters refused: {refusal:?}"
        )));
    }
    keep(cx, role, name, &counted);
    Ok(())
}

/// What the previous run left: each board's snapshot before its bring-up.
fn cold_snapshots(cx: &mut Cx<'_>) -> Result<(), Failure> {
    for role in [RECEIVER, SENDER] {
        let readings = snapshot(cx, role, Snapshot::Cold)?;
        keep(cx, role, "snapshot-cold", &readings);
    }
    Ok(())
}

/// Both boards up and receiving, then their snapshots and counters after it.
fn bring_up(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let mut stations = [[0_u8; 6]; 2];
    for (role, station) in [RECEIVER, SENDER].into_iter().zip(&mut stations) {
        match cx.device(role)?.call(BringUp, BRING_UP)? {
            BroughtUp::Up { station: up, .. } => *station = up,
            other => return Err(Failure::Fail(format!("{role}: bring-up: {other:?}"))),
        }
    }
    let [receiver, sender] = stations;
    *ADDRESSES.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(Addresses { receiver, sender });
    for role in [RECEIVER, SENDER] {
        let readings = snapshot(cx, role, Snapshot::Up)?;
        keep(cx, role, "snapshot-up", &readings);
        counters(cx, role, "counters-up")?;
    }
    Ok(())
}

fn addresses() -> Result<Addresses, Failure> {
    ADDRESSES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ok_or_else(|| Failure::Incomplete("no bring-up gave the addresses".to_owned()))
}

/// Each board told its peer: the sender its receiver, the receiver its
/// sender, by the addresses their bring-ups answered.
fn peer(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let Addresses { receiver, sender } = addresses()?;
    match cx.device(SENDER)?.call(Target { receiver }, STEP)? {
        Targeted::Targeted => {}
        other => return Err(Failure::Fail(format!("sender: target: {other:?}"))),
    }
    match cx.device(RECEIVER)?.call(Expect { sender }, STEP)? {
        Expecting::Expecting => Ok(()),
        other => Err(Failure::Fail(format!("receiver: expect: {other:?}"))),
    }
}

/// The hand-over's rounds.
const HAND_OVERS: usize = 64;

/// The least, the median and the greatest of `spans`.
fn spread(spans: &mut [u32]) -> [u32; 3] {
    spans.sort_unstable();
    let least = spans.first().copied().unwrap_or_default();
    let median = spans.get(spans.len() / 2).copied().unwrap_or_default();
    let greatest = spans.last().copied().unwrap_or_default();
    [least, median, greatest]
}

/// The receiver's command that does no work, `HAND_OVERS` times: what one
/// crossing of the radio's hand-over costs there and back, in the hart's
/// cycles. Passes when every round is answered with its spans.
fn hand_over(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (mut down, mut held, mut up) = (Vec::new(), Vec::new(), Vec::new());
    let mut rate = 0;
    for round in 0..HAND_OVERS {
        match cx.device(RECEIVER)?.call(HandOver, STEP)? {
            HandedOver::Spans {
                down: d,
                held: h,
                up: u,
                cycles_per_micro,
            } => {
                down.push(d);
                held.push(h);
                up.push(u);
                rate = cycles_per_micro;
            }
            other => {
                return Err(Failure::Fail(format!(
                    "receiver: hand-over {round}: {other:?}"
                )));
            }
        }
    }
    keep(cx, RECEIVER, "hand-over-down-cycles", &spread(&mut down));
    keep(cx, RECEIVER, "hand-over-held-cycles", &spread(&mut held));
    keep(cx, RECEIVER, "hand-over-up-cycles", &spread(&mut up));
    keep(cx, RECEIVER, "cycles-per-micro", &rate);
    Ok(())
}

/// How long a step of `millis` may take to answer.
fn within(millis: u32) -> Duration {
    Duration::from_millis(u64::from(millis)).saturating_add(STEP)
}

/// A window's ring anomalies of every kind.
const fn anomalies(counts: &service::Counts) -> u32 {
    counts
        .length_anomalies
        .saturating_add(counts.doorbell_anomalies)
}

/// Case C's rounds on the sender while the receiver serves R1 to R4; each
/// R's verdict is recorded, and the case fails naming every R that did.
fn case_c(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rounds = cx.device(SENDER)?.post(Round { rounds: ROUNDS })?;
    let mut timeline = Timeline::new();
    let mut failed = Vec::new();
    // R1 and R2: one window, singles lent and chains copied.
    let began = timeline.now();
    let windowed = cx.device(RECEIVER)?.call(
        Window {
            millis: WINDOW_MS,
            record: Record::Chains,
        },
        within(WINDOW_MS),
    )?;
    timeline.step("r1-r2-window", began);
    keep(cx, RECEIVER, "r1-r2-window", &windowed);
    counters(cx, RECEIVER, "counters-r1-r2")?;
    match &windowed {
        Windowed::Counted {
            counts,
            spares,
            chains,
            ..
        } => {
            let anomalies = anomalies(counts);
            if !(counts.singles > 0
                && counts.sender >= R1_UNITS
                && counts.mismatched == 0
                && counts.lengths == 0
                && anomalies == 0
                && *spares == SPARES)
            {
                failed.push(format!("R1: {counts:?}, {spares} spares"));
            }
            let long = LONG_TAGS.iter().all(|tag| {
                chains.iter().any(|chain| {
                    chain.tag == *tag && chain.chains >= R2_CHAINS && chain.mismatched == 0
                })
            });
            if !(long && anomalies == 0) {
                failed.push(format!("R2: {chains:?}"));
            }
        }
        other => failed.push(format!("R1 and R2: {other:?}")),
    }
    // R3: the stall, the walker's standing, then the exit by serve.
    let began = timeline.now();
    let stood = cx
        .device(RECEIVER)?
        .call(Stall { millis: STALL_MS }, within(STALL_MS))?;
    timeline.step("r3-stall", began);
    keep(cx, RECEIVER, "r3-standing", &stood);
    let began = timeline.now();
    let exit = cx.device(RECEIVER)?.call(
        Window {
            millis: EXIT_MS,
            record: Record::Counts,
        },
        within(EXIT_MS),
    )?;
    timeline.step("r3-window", began);
    keep(cx, RECEIVER, "r3-window", &exit);
    match (&stood, &exit) {
        (Stood::Standing(standing), Windowed::Counted { counts, .. }) => {
            let state = match (standing.next, standing.tail_word) {
                (service::NextAt::Zero, service::TailWord::Completed) => Some(0),
                (service::NextAt::Ring(at), service::TailWord::Armed) if at == standing.tail => {
                    Some(1)
                }
                _ => None,
            };
            keep(cx, RECEIVER, "r3-state", &state);
            let anomalies = anomalies(counts);
            if !(standing.cursor == service::Cursor::Steady
                && state.is_some()
                && counts.sender >= R3_UNITS
                && counts.mismatched == 0
                && anomalies == 0)
            {
                failed.push(format!("R3: {standing:?}, {counts:?}"));
            }
        }
        (stood, exit) => failed.push(format!("R3: {stood:?}, {exit:?}")),
    }
    // R4: the turns.
    let mut turns = Vec::new();
    for turn in 0..TURNS {
        let began = timeline.now();
        let turned = cx.device(RECEIVER)?.call(
            Turn { millis: TURN_MS },
            within(TURN_MS).saturating_add(Duration::from_millis(u64::from(TURN_MS))),
        )?;
        timeline.step(&format!("r4-turn-{turn}"), began);
        let pass = match &turned {
            Turned::Turned {
                before,
                after,
                handles,
                ..
            } => {
                after.sender >= R4_UNITS
                    && before.sender > 0
                    && *handles == BUFFERS
                    && after.mismatched == 0
                    && anomalies(before) == 0
                    && anomalies(after) == 0
            }
            _ => false,
        };
        if !pass {
            failed.push(format!("R4 turn {turn}: {turned:?}"));
        }
        let ended = !matches!(turned, Turned::Turned { .. });
        turns.push(turned);
        if ended {
            break;
        }
    }
    keep(cx, RECEIVER, "r4-turns", &turns);
    counters(cx, RECEIVER, "counters-r4")?;
    // The sender's rounds.
    let sent = cx.device(SENDER)?.wait(rounds, ROUNDS_TIMEOUT)?;
    keep(cx, SENDER, "rounds", &sent);
    counters(cx, SENDER, "counters-rounds")?;
    keep(cx, RECEIVER, "steps", &timeline.steps);
    match &sent {
        Sent::Sent { .. } => {}
        Sent::Failed(stopped) => {
            // The sender's clock counts from its step's start, the host's from
            // the post: the two differ by the request's way to the board.
            let at = u64::from(stopped.micros / 1_000);
            let during = timeline.during(at);
            keep(
                cx,
                SENDER,
                "rounds-failure",
                &serde_json::json!({ "millis": at, "receiver_step": during }),
            );
            failed.push(format!(
                "case C: {stopped:?}, the receiver at {}",
                during.unwrap_or("no step")
            ));
        }
        other => failed.push(format!("case C: {other:?}")),
    }
    if failed.is_empty() {
        return Ok(());
    }
    // A failure is what the pair studies: both boards still answer their
    // snapshot and counters; the stop case stops them.
    for role in [RECEIVER, SENDER] {
        let readings = snapshot(cx, role, Snapshot::Up)?;
        keep(cx, role, "snapshot-after-failure", &readings);
        counters(cx, role, "counters-after-failure")?;
    }
    Err(Failure::Fail(failed.join("; ")))
}

/// The receiver's steps, each from its request to its answer, in
/// milliseconds since the sender's rounds were posted.
struct Timeline {
    posted: Instant,
    steps: Vec<(String, u64, u64)>,
}

impl Timeline {
    fn new() -> Self {
        Self {
            posted: Instant::now(),
            steps: Vec::new(),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.posted.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn step(&mut self, name: &str, began: u64) {
        let ended = self.now();
        self.steps.push((name.to_owned(), began, ended));
    }

    /// The step that ran at `millis`, or none.
    fn during(&self, millis: u64) -> Option<&str> {
        self.steps
            .iter()
            .find(|(_, began, ended)| (*began..=*ended).contains(&millis))
            .map(|(name, ..)| name.as_str())
    }
}

/// Both boards stop: the walker's edge, the stop request and its hold, the
/// reset after a stop, the release.
fn stop(cx: &mut Cx<'_>) -> Result<(), Failure> {
    for role in [RECEIVER, SENDER] {
        match cx.device(role)?.call(Stop, STEP)? {
            Stopped::Stopped { micros } => {
                keep(cx, role, "stop-micros", &micros);
            }
            other => return Err(Failure::Fail(format!("{role}: stop: {other:?}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_s_moment_names_the_receiver_s_step_that_ran_then() {
        let mut timeline = Timeline::new();
        timeline.steps = vec![
            ("r1-r2-window".to_owned(), 5, 2_010),
            ("r3-stall".to_owned(), 2_015, 2_120),
            ("r4-turn-0".to_owned(), 2_130, 2_600),
        ];
        assert_eq!(timeline.during(2_100), Some("r3-stall"));
        assert_eq!(timeline.during(2_010), Some("r1-r2-window"));
        assert_eq!(timeline.during(2_125), None, "between two steps");
        assert_eq!(timeline.during(9_000), None, "after the last");
    }
}
