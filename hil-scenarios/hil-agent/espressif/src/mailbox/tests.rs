use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::*;

fn identity(request_id: u32) -> RequestIdentity {
    RequestIdentity {
        boot_id: 7,
        session_id: 1,
        request_id,
    }
}

#[test]
fn a_second_request_is_busy_until_the_worker_takes_the_first() {
    let mut mailbox = Mailbox::<u8>::new();
    let (outbox, mut inbox) = mailbox.split();
    assert_eq!(outbox.hand(identity(1), 10), Ok(()));
    assert_eq!(outbox.hand(identity(2), 20), Err(Busy));

    let mut context = Context::from_waker(Waker::noop());
    let next = pin!(inbox.next());
    assert_eq!(next.poll(&mut context), Poll::Ready((identity(1), 10)));
    assert_eq!(outbox.hand(identity(3), 30), Ok(()));
}

#[test]
fn the_worker_waits_for_the_next_request_and_takes_it_once_handed() {
    let mut mailbox = Mailbox::<u8>::new();
    let (outbox, mut inbox) = mailbox.split();
    let mut context = Context::from_waker(Waker::noop());
    let mut next = pin!(inbox.next());
    assert_eq!(next.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(outbox.hand(identity(4), 40), Ok(()));
    assert_eq!(next.poll(&mut context), Poll::Ready((identity(4), 40)));
}
