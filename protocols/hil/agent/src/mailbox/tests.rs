use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::*;
use crate::test_messages::{Probe, ProbeArmed, ProbeMode};

crate::requests! {
    enum Probes (sessions = false) {
        Test(Probe),
    }
}

fn identity(request_id: u32) -> RequestIdentity {
    RequestIdentity {
        boot_id: 7,
        session_id: 1,
        request_id,
    }
}

fn poll<F: core::future::Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn the_worker_waits_for_a_request_and_its_reply_names_it() {
    let mut mailbox = Mailbox::<Probes>::new();
    let (console, mut inbox) = mailbox.split();
    let mut next = pin!(inbox.next());
    assert!(poll(next.as_mut()).is_pending());
    assert!(
        console
            .requests
            .try_send((identity(4), Probes::Test(Probe(ProbeMode::Stall))))
            .is_ok()
    );
    let Poll::Ready((Probes::Test(Probe(mode)), reply)) = poll(next.as_mut()) else {
        panic!("the request is handed");
    };
    assert_eq!((mode, reply.identity()), (ProbeMode::Stall, identity(4)));
    reply.answer(&ProbeArmed(mode));
    let answered = console.answers.try_receive().expect("the answer");
    assert_eq!(answered.identity, identity(4));
    assert!(matches!(
        answered.outcome,
        Outcome::Encoded(ref message, After::Nothing)
            if message.request_id == 4 && message.session_id == 1 && message.key == ProbeArmed::KEY
    ));
}

#[test]
fn a_dropped_reply_leaves_its_request_owed_and_the_next_request_pays_it() {
    let mut mailbox = Mailbox::<Probes>::new();
    let (console, mut inbox) = mailbox.split();
    assert!(
        console
            .requests
            .try_send((identity(5), Probes::Test(Probe(ProbeMode::Complete))))
            .is_ok()
    );
    let Poll::Ready((_, reply)) = poll(pin!(inbox.next())) else {
        panic!("the request is handed");
    };
    drop(reply);
    assert!(
        console.answers.try_receive().is_none(),
        "nothing is sent on the drop"
    );
    let mut next = pin!(inbox.next());
    assert!(poll(next.as_mut()).is_pending(), "no next request yet");
    let answered = console.answers.try_receive().expect("the debt is paid");
    assert_eq!(answered.identity, identity(5));
    assert!(matches!(answered.outcome, Outcome::Unanswered));
    assert!(
        console
            .requests
            .try_send((identity(6), Probes::Test(Probe(ProbeMode::Complete))))
            .is_ok()
    );
    let Poll::Ready((_, reply)) = poll(next.as_mut()) else {
        panic!("the next request is handed");
    };
    assert_eq!(reply.identity(), identity(6));
    reply.answer(&ProbeArmed(ProbeMode::Complete));
    assert!(matches!(
        console
            .answers
            .try_receive()
            .map(|answered| answered.outcome),
        Some(Outcome::Encoded(_, After::Nothing))
    ));
}
