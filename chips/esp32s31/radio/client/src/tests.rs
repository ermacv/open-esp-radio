use super::*;

#[test]
fn the_static_cells_are_claimed_once() {
    assert!(claim().is_some());
    assert!(claim().is_none());
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
    const RADIO: oer_esp32s31_hal::domain::RadioClient =
        oer_esp32s31_hal::domain::RadioClient::Wifi;
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
    let poster = slot.claim().expect("a fresh slot has its poster");
    (
        MembershipCell { poster },
        ClientInbox { slot },
        MemberMint::new(),
    )
}

fn complete<O: sealed::AnswerPart<TestClient>>(
    mut pending: Pending<TestClient, O>,
    inbox: ClientInbox<TestClient>,
    answer: Answer<TestClient>,
) -> (MembershipCell<TestClient>, O) {
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(Pin::new(&mut pending).poll(&mut cx).is_pending());
    let (_, responder) = inbox.take().expect("the client posted its request");
    responder.answer(answer);
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
    let (cell, answer) = complete(
        cell.join(11),
        inbox,
        Answer::Join(Ok(Joined {
            member: mint.untracked(),
            epoch,
            payload: 12,
        })),
    );
    let joined = answer.expect("the actor answered with a successful join");
    assert_eq!((joined.payload, joined.epoch), (12, epoch));
    let _next = cell.track(joined.member, 13);
}

#[test]
fn pending_track_returns_the_track_answer_and_cell() {
    let (cell, inbox, mint) = client();
    let (cell, answer) = complete(
        cell.track(mint.untracked(), 21),
        inbox,
        Answer::Track(Ok(TrackedMember {
            member: mint.track(mint.untracked()),
            payload: 22,
        })),
    );
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
        Answer::Retune(Ok(RetunedMember {
            member: mint.track(mint.untracked()),
            payload: 32,
        })),
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
        Answer::Leave(Left {
            payload: 42,
            domain: DomainHealth::Poisoned(POISON),
        }),
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

fn left() -> Answer<TestClient> {
    Answer::Leave(Left {
        payload: 51,
        domain: DomainHealth::Healthy,
    })
}

#[test]
#[should_panic(expected = "the actor answers a join with a join answer")]
fn pending_join_rejects_another_answer_variant() {
    let (cell, inbox, _) = client();
    let _ = complete(cell.join(11), inbox, left());
}

#[test]
#[should_panic(expected = "the actor answers a track with a track answer")]
fn pending_track_rejects_another_answer_variant() {
    let (cell, inbox, mint) = client();
    let _ = complete(cell.track(mint.untracked(), 21), inbox, left());
}

#[test]
#[should_panic(expected = "the actor answers a retune with a retune answer")]
fn pending_retune_rejects_another_answer_variant() {
    let (cell, inbox, mint) = client();
    let _ = complete(cell.retune(mint.track(mint.untracked()), 31), inbox, left());
}

#[test]
#[should_panic(expected = "the actor answers a leave with a leave answer")]
fn pending_leave_rejects_another_answer_variant() {
    let (cell, inbox, mint) = client();
    let _ = complete(
        cell.leave(Leaving::Untracked(mint.untracked()), 41),
        inbox,
        Answer::Join(Err(Failed {
            returned: 51,
            cause: Failure::Refused,
        })),
    );
}
