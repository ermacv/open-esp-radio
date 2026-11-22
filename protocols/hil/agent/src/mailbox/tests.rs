use core::pin::pin;
use core::task::{Context, Poll, Waker};

use oer_portable_memory::Answered;

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
fn a_reply_names_its_request_and_encodes_the_answer_under_its_identity() {
    let mut mailbox = Mailbox::<Probes>::new();
    let (mut console, mut inbox) = mailbox.split();
    assert!(
        console
            .0
            .hand((identity(4), Probes::Test(Probe(ProbeMode::Stall))))
            .is_ok()
    );
    let Poll::Ready((Probes::Test(Probe(mode)), reply)) = poll(pin!(inbox.next())) else {
        panic!("the request is handed");
    };
    assert_eq!((mode, reply.identity()), (ProbeMode::Stall, identity(4)));
    reply.answer(&ProbeArmed(mode));
    let Poll::Ready(answered) = poll(pin!(console.0.answered())) else {
        panic!("the answer comes back");
    };
    assert!(matches!(
        answered,
        Answered::Given(Outcome::Encoded(ref message, After::Nothing))
            if message.request_id == 4 && message.session_id == 1 && message.key == ProbeArmed::KEY
    ));
}
