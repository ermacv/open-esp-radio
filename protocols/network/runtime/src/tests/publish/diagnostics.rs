//! The switchable diagnostics over the service's publishing: off they ask
//! once a turn and count nothing, read no clock; on they count walks, ends
//! told and frames built at the points that do that work, and the same
//! script decides the same either way.

use super::super::probe::Probe;
use super::*;
use crate::{Diagnosis, STAGES, Stage};

/// What a script decided: every frame the port took, every result, and every
/// step the sink heard, turns marked.
type Decided = (Vec<Vec<u8>>, Vec<Sent>, Vec<Step>);

/// The refusal in the middle of a turn, then everything sent: the steps a
/// turn's publishing meets most (frames, a refusal, frames after it).
fn refusal_script<Net: 'static>(diagnostics: Probe) -> Decided {
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_diagnosed::<Net, _>(diagnostics);
    let mut events = TurnMarks(policy.clone());
    let drive = async {
        let observers = queue_four_each(&mut sockets, &wire).await;
        take_then_fill(&wire, 5);
        settle().await;
        open_port(&wire, false);
        let mut results = Vec::new();
        for observer in observers {
            results.push(result_of(observer).await);
        }
        stop.try_send(()).ok().unwrap();
        results
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (mut ended, results) = run_tasks(clock, serve, drive);
    home(&mut ended, &mut sockets);
    let sent = wire.0.borrow().sent.clone();
    let steps = policy.0.borrow().clone();
    (sent, results, steps)
}

#[test]
fn off_diagnostics_count_nothing_and_read_no_clock() {
    brand!(Net);
    let probe = Probe::default();
    let _decided = refusal_script::<Net>(probe.clone());
    let probed = probe.0.borrow();
    assert!(probed.asked > 0, "each turn asks");
    assert_eq!(probed.clock, 0, "no clock read");
    assert_eq!(probed.adds, 0, "no count handed over");
    assert_eq!(probed.total, Diagnosis::default());
}

#[test]
fn the_same_script_decides_the_same_with_diagnostics_on_and_off() {
    brand!(Off);
    brand!(On);
    let off = refusal_script::<Off>(Probe::default());
    let probe = Probe::on();
    let on = refusal_script::<On>(probe.clone());
    assert_eq!(on.0, off.0, "the same frames, in the same order");
    assert_eq!(on.1, off.1, "the same results");
    assert_eq!(on.2, off.2, "the same frames and publishing ends");
    assert_ne!(probe.total(), Diagnosis::default(), "and on, it counted");
}

#[test]
fn publishing_counts_its_walks_ends_and_frames() {
    brand!(Net);
    let probe = Probe::on();
    let (sent, results, steps) = refusal_script::<Net>(probe.clone());
    let total = probe.total();
    let count = |wanted: fn(&Step) -> bool| {
        u32::try_from(steps.iter().filter(|s| wanted(s)).count()).unwrap()
    };
    assert_eq!(total.turns, count(|step| *step == Step::Turn));
    // The port refuses for room before any fill, so every frame it was
    // asked to fill is one it took.
    assert_eq!(total.materialised, u32::try_from(sent.len()).unwrap());
    // Each datagram's end is told once, by its socket's ended chain.
    assert_eq!(results.len(), 8);
    assert_eq!(total.told, 9, "the first datagram and the eight after it");
    // Each sent datagram was visited by the walk that chose it; a walk is
    // one a frame or a publishing's end, and none visits more than every
    // place of both sockets.
    assert!(total.walked >= 9, "{total:?}");
    let walks =
        count(|step| *step == Step::Accepted) + count(|step| matches!(step, Step::Published(_)));
    assert!(
        total.walked <= walks * u32::try_from(2 * PLAN_CREDITS).unwrap(),
        "{total:?}"
    );
}

#[test]
fn each_turn_s_stages_take_the_clock_between_their_ends() {
    brand!(Net);
    let probe = Probe::on();
    let _decided = refusal_script::<Net>(probe.clone());
    let probed = probe.0.borrow();
    let total = probed.total;
    let turns = u64::from(total.turns);
    // The probe's clock moves one tick a read: a turn reads it at its start
    // and at each of its stages' ends; a receive turn has no notices and no
    // publication, so four stages, the others six.
    let receive = turns - total.ticks(Stage::Notices);
    for stage in [Stage::Intake, Stage::Returns, Stage::Receive, Stage::Settle] {
        assert_eq!(total.ticks(stage), turns, "{stage:?}");
    }
    for stage in [Stage::Notices, Stage::Publish] {
        assert_eq!(total.ticks(stage), turns - receive, "{stage:?}");
    }
    assert_eq!(total.ticks(Stage::Waiting), 0, "no returns came to a wait");
    assert_eq!(probed.clock, 7 * turns - 2 * receive);
    assert_eq!(
        total.stage_ticks.iter().sum::<u64>(),
        6 * turns - 2 * receive,
        "{STAGES} stages"
    );
}

#[test]
fn diagnostics_switched_on_count_from_the_next_turn() {
    brand!(Net);
    let probe = Probe::default();
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_diagnosed::<Net, _>(probe.clone());
    let mut events = TurnMarks(policy.clone());
    let switched = core::cell::Cell::new(0);
    let drive = async {
        let observers = queue_four_each(&mut sockets, &wire).await;
        switched.set(policy.0.borrow().len());
        probe.switch(true);
        open_port(&wire, false);
        for observer in observers {
            assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        }
        settle().await;
        probe.switch(false);
        let off = policy.0.borrow().len();
        // Off again: the next turns count nothing.
        let _late = plan_send(&mut sockets[0], b"late").await;
        settle().await;
        stop.try_send(()).ok().unwrap();
        off
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (_ended, off) = run_tasks(clock, serve, drive);
    let steps = policy.0.borrow();
    let on_turns = steps[switched.get()..off]
        .iter()
        .filter(|step| **step == Step::Turn)
        .count();
    let total = probe.total();
    assert!(on_turns > 0);
    assert_eq!(total.turns, u32::try_from(on_turns).unwrap());
    assert_eq!(
        total.told, 8,
        "the eight ends told while on, not the first's"
    );
}
