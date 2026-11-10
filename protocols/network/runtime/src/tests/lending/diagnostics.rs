//! The switchable diagnostics over lending: a refusal for want of credit
//! counts the loans out, and return servicing counts its batches and the
//! credits it restored, in a turn or in a pass of the wait.

use super::super::probe::Probe;
use super::*;
use crate::Stage;

/// A lent datagram held, a second refused for credit, the first dropped in
/// a wait, a third lent on the returned credit and dropped: the counts, and
/// the core's drops.
fn credit_script<Net: 'static>(
    probe: Probe,
) -> (crate::Diagnosis, Vec<NetEvent>, Vec<ServiceEvent>) {
    let (mut service, _copying, mut lending, wire, stop) =
        lending_diagnosed::<Net, _>(4, 1, probe.clone());
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let tape = Tape::default();
    let mut events = Both(Drops::default(), tape.clone());
    let drive = async {
        arrive(&wire, LENDING, b"first");
        let first = lending.receive().await.unwrap();
        arrive(&wire, LENDING, b"no credit");
        settle().await;
        drop(first);
        settle().await;
        arrive(&wire, LENDING, b"second");
        let Received::Lent(second) = lending.receive().await.unwrap() else {
            panic!("lent once the first came back");
        };
        drop(second);
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
    let mut ended = service.retire();
    ended.collect();
    assert_eq!(lent_out(&wire), 0);
    let tape = tape.0.borrow().clone();
    (probe.total(), events.0.0, tape)
}

/// The core's drops and the service's events, apart.
struct Both(Drops, Tape);

impl EventSink<NetEvent> for Both {
    fn event(&mut self, event: NetEvent) {
        self.0.event(event);
    }
}

impl EventSink<ServiceEvent> for Both {
    fn event(&mut self, event: ServiceEvent) {
        self.1.event(event);
    }
}

#[test]
fn a_refusal_for_credit_counts_the_loans_then_out() {
    #[derive(Debug)]
    struct Net;
    let (total, drops, _) = credit_script::<Net>(Probe::on());
    assert_eq!(drops, [NetEvent::NotLent(NotLent::NoCredit)]);
    assert_eq!(total.no_credit, 1);
    assert_eq!(total.no_credit_loans, 1, "the socket's one credit was out");
    assert_eq!(
        total.no_credit_unreturned, 0,
        "the held buffer had not come back"
    );
}

#[test]
fn return_servicing_counts_its_batches_and_the_credits_it_restored() {
    #[derive(Debug)]
    struct Net;
    let probe = Probe::on();
    let (total, _, tape) = credit_script::<Net>(probe.clone());
    let waiting = tape
        .iter()
        .filter(|event| matches!(event, ServiceEvent::Returned { .. }))
        .count();
    assert_eq!(waiting, 2, "each drop was taken back by a pass of the wait");
    assert_eq!(total.return_batches, 2);
    assert_eq!(total.credits_restored, 2, "both lent buffers' loans");
    assert_eq!(total.blocks_returned, 0, "no block came by a return");
    // A pass reads the clock around its return servicing.
    assert_eq!(total.ticks(Stage::Waiting), 2);
    assert_eq!(
        probe.0.borrow().clock,
        7 * u64::from(total.turns) + 2 * 2,
        "seven reads a turn, two a pass with returns"
    );
}

#[test]
fn off_lending_diagnostics_count_nothing_and_change_nothing() {
    #[derive(Debug)]
    struct Off;
    #[derive(Debug)]
    struct On;
    let probe = Probe::default();
    let off = credit_script::<Off>(probe.clone());
    let probed = probe.0.borrow();
    assert!(probed.asked > 0);
    assert_eq!((probed.clock, probed.adds), (0, 0));
    let on = credit_script::<On>(Probe::on());
    assert_eq!(on.1, off.1, "the same drops");
    assert_eq!(on.2, off.2, "the same turns, passes and returns");
}
