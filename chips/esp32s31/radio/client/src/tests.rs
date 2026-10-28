use super::*;

#[test]
fn each_cells_private_slots_are_claimed_together_once() {
    extern crate std;
    let doorbell = std::boxed::Box::leak(std::boxed::Box::new(Doorbell::new()));
    let slot = std::boxed::Box::leak(std::boxed::Box::new(ClientSlot::<TestClient>::new(
        doorbell, 1,
    )));
    assert!(slot.claim().is_some());
    assert!(slot.claim().is_none());
}

#[test]
fn epochs_are_minted_in_order() {
    let mint = MemberMint::new();
    let first = mint.next_epoch(None);
    let second = mint.next_epoch(Some(first));
    assert_eq!((first.get(), second.get()), (0, 1));
    assert_ne!(first, second);
}

#[derive(Debug)]
enum TestClient {}

impl sealed::Sealed for TestClient {}

impl Client for TestClient {
    type Join = u32;
    type Joined = u32;
    type JoinReturned = u32;
    type Track = u32;
    type Tracked = u32;
    type TrackReturned = u32;
    type Retune = u32;
    type Retuned = u32;
    type RetuneReturned = u32;
    type Leave = u32;
    type Left = u32;

    fn join_returned(join: u32) -> u32 {
        join
    }

    fn track_returned(track: u32) -> u32 {
        track
    }

    fn retune_returned(retune: u32) -> u32 {
        retune
    }
}

fn client() -> (
    MembershipCell<TestClient>,
    ClientInbox<TestClient>,
    MemberMint,
) {
    extern crate std;
    use std::boxed::Box;

    let doorbell = Box::leak(Box::new(Doorbell::new()));
    let slot = Box::leak(Box::new(ClientSlot::new(doorbell, 1)));
    let cell = slot.claim().expect("a fresh slot has its cell");
    (cell, ClientInbox { slot }, MemberMint::new())
}

fn complete<O: sealed::Reply<TestClient>>(
    mut pending: Pending<TestClient, O>,
    inbox: ClientInbox<TestClient>,
    answer: impl FnOnce(Request<TestClient>),
) -> (MembershipCell<TestClient>, O) {
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(Pin::new(&mut pending).poll(&mut cx).is_pending());
    answer(inbox.take().expect("the client posted its request"));
    assert!(inbox.take().is_none());
    let Poll::Ready(result) = Pin::new(&mut pending).poll(&mut cx) else {
        panic!("the actor's answer makes the pending request ready");
    };
    assert!(Pin::new(&mut pending).poll(&mut cx).is_pending());
    result
}

#[test]
fn pending_join_returns_the_join_answer_and_cell() {
    let (cell, inbox, mint) = client();
    let epoch = mint.next_epoch(None);
    let (cell, answer) = complete(cell.join(11), inbox, |request| {
        let Request::Join(payload, responder) = request else {
            panic!("join request")
        };
        assert_eq!(payload, 11);
        responder.answer(Ok(Joined {
            member: mint.untracked(),
            epoch,
            payload: 12,
        }));
    });
    let joined = answer.expect("the actor answered with a successful join");
    assert_eq!((joined.payload, joined.epoch), (12, epoch));
    let _next = cell.track(joined.member, 13);
}

#[test]
fn pending_track_returns_the_track_answer_and_cell() {
    let (cell, inbox, mint) = client();
    let (cell, answer) = complete(cell.track(mint.untracked(), 21), inbox, |request| {
        let Request::Track(member, payload, responder) = request else {
            panic!("track request")
        };
        assert_eq!(payload, 21);
        responder.answer(Ok(TrackedMember {
            member: mint.track(member),
            payload: 22,
        }));
    });
    let tracked = answer.expect("the actor answered with successful tracking");
    assert_eq!(tracked.payload, 22);
    let _next = cell.retune(tracked.member, 23);
}

#[test]
fn pending_retune_returns_the_retune_answer_and_cell() {
    let (cell, inbox, mint) = client();
    let (cell, answer) = complete(
        cell.retune(mint.track(mint.untracked()), 31),
        inbox,
        |request| {
            let Request::Retune(member, payload, responder) = request else {
                panic!("retune request")
            };
            assert_eq!(payload, 31);
            responder.answer(Ok(RetunedMember {
                member,
                payload: 32,
            }));
        },
    );
    let retuned = answer.expect("the actor answered with a successful retune");
    assert_eq!(retuned.payload, 32);
    let _next = cell.leave(Leaving::Tracked(retuned.member), 33);
}

#[test]
fn pending_leave_returns_the_leave_answer_and_cell() {
    let (cell, inbox, mint) = client();
    let (cell, left) = complete(
        cell.leave(Leaving::Untracked(mint.untracked()), 41),
        inbox,
        |request| {
            let Request::Leave(_, payload, responder) = request else {
                panic!("leave request")
            };
            assert_eq!(payload, 41);
            responder.answer(Left {
                payload: 42,
                domain: DomainHealth::Poisoned(POISON),
            });
        },
    );
    assert_eq!(
        (left.payload, left.domain),
        (42, DomainHealth::Poisoned(POISON))
    );
    let _next = cell.join(43);
}

const POISON: Poison = Poison {
    origin: PoisonOrigin::TrackingTick,
    cause: PoisonCause::Tracking(PhyFault::EdgeTimedOut),
};

#[test]
fn refused_requests_return_the_original_payload_membership_and_cell() {
    let (cell, inbox, _) = client();
    let (cell, joined) = complete(cell.join(71), inbox, |request| {
        let Request::Join(payload, responder) = request else {
            panic!("join")
        };
        responder.answer(Err(Failed {
            returned: payload,
            cause: Failure::Refused,
        }));
    });
    assert_eq!(joined.err().expect("refused join").returned, 71);

    let (cell2, inbox, mint) = client();
    let (cell2, tracked) = complete(cell2.track(mint.untracked(), 72), inbox, |request| {
        let Request::Track(member, payload, responder) = request else {
            panic!("track")
        };
        responder.answer(Err(Failed {
            returned: (member, payload),
            cause: Failure::Refused,
        }));
    });
    let (member, payload) = tracked.err().expect("refused track").returned;
    assert_eq!(payload, 72);
    let _retry = cell2.track(member, payload);

    let (cell3, inbox, mint) = client();
    let (cell3, retuned) = complete(
        cell3.retune(mint.track(mint.untracked()), 73),
        inbox,
        |request| {
            let Request::Retune(member, payload, responder) = request else {
                panic!("retune")
            };
            responder.answer(Err(Failed {
                returned: (member, payload),
                cause: Failure::Poisoned(POISON),
            }));
        },
    );
    let (member, payload) = retuned.err().expect("refused retune").returned;
    assert_eq!(payload, 73);
    let _retry = cell3.retune(member, payload);
    let _retry = cell.join(71);
}
