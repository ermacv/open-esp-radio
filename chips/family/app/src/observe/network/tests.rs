use super::*;

/// One turn of a sender: `took` requests, `spent` sockets left with more,
/// `accepted` frames, publishing ended by `publish`.
fn turn(
    tally: &mut TurnTally,
    counter: &NetworkCounter,
    took: &[Request],
    spent: u32,
    accepted: u32,
    publish: Publish,
) {
    for request in took {
        tally.took(counter, *request);
    }
    for _ in 0..spent {
        tally.budget_spent();
    }
    for _ in 0..accepted {
        tally.accepted();
    }
    tally.published(publish);
    tally.ended(counter, true, true, 0);
}

#[test]
fn each_request_counts_by_its_kind() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    use Request::*;
    turn(
        &mut tally,
        &counter,
        &[
            Reserve { blocks: 1 },
            Reserve { blocks: 16 },
            Send,
            Release,
            Close,
        ],
        0,
        0,
        Publish::Chose,
    );
    let read = counter.read();
    assert_eq!(
        (read.reserves, read.sends, read.releases, read.closes),
        (2, 1, 1, 1)
    );
    assert_eq!(read.turns, 1);
}

#[test]
fn a_reservation_counts_the_blocks_it_asks_for_and_fills_one_bin() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    let asked = [1, 2, 3, 4, 5, 8, 9, 16, 16];
    let took = asked.map(|blocks| Request::Reserve { blocks });
    turn(&mut tally, &counter, &took, 0, 0, Publish::Chose);
    let read = counter.read();
    assert_eq!(read.reserves, 9, "one per message");
    assert_eq!(read.reserve_blocks, 64, "the blocks they asked for");
    assert_eq!(read.reserve_by_size, [1, 1, 2, 2, 3]);
    assert_eq!(read.sends, 0);
}

#[test]
fn a_spent_budget_counts_per_socket_and_per_turn() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    turn(&mut tally, &counter, &[], 2, 0, Publish::Chose);
    turn(&mut tally, &counter, &[], 0, 0, Publish::Chose);
    turn(&mut tally, &counter, &[], 1, 0, Publish::Chose);
    let read = counter.read();
    assert_eq!(
        (read.turns, read.budget_spent, read.budget_turns),
        (3, 3, 2)
    );
}

#[test]
fn accepted_frames_sum_and_fill_one_bin_per_turn() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    for accepted in [0, 1, 2, 2, 3, 4, 5, 8, 9, 12] {
        turn(&mut tally, &counter, &[], 0, accepted, Publish::Chose);
    }
    let read = counter.read();
    assert_eq!(read.accepted, 46);
    assert_eq!(read.accepted_by_turn, [1, 1, 2, 1, 1, 2, 2]);
    assert_eq!(read.turns, 10);
}

#[test]
fn a_turn_counts_how_publishing_ended_and_what_was_out() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    for publish in [
        Publish::Chose,
        Publish::Refused,
        Publish::Refused,
        Publish::Budget,
        Publish::Deferred,
        Publish::Fault,
    ] {
        tally.published(publish);
        tally.ended(&counter, true, true, 0);
    }
    tally.published(Publish::Chose);
    tally.ended(&counter, false, true, 0);
    tally.published(Publish::Chose);
    tally.ended(&counter, true, false, 0);
    let read = counter.read();
    assert_eq!(
        [
            read.publish_chose,
            read.publish_refused,
            read.publish_budget,
            read.publish_deferred,
            read.publish_fault,
        ],
        [3, 2, 1, 1, 1]
    );
    assert_eq!((read.blocks_out, read.credits_out), (1, 1));
}

#[test]
fn a_yield_counts_whether_its_turn_left_work() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    // A spent budget leaves work.
    turn(&mut tally, &counter, &[], 1, 0, Publish::Chose);
    tally.yielded(&counter);
    // So does a full intake.
    tally.intake_full();
    tally.published(Publish::Chose);
    tally.ended(&counter, true, true, 0);
    tally.yielded(&counter);
    // And publishing at its budget, or deferred.
    turn(&mut tally, &counter, &[], 0, 8, Publish::Budget);
    tally.yielded(&counter);
    turn(&mut tally, &counter, &[], 0, 0, Publish::Deferred);
    tally.yielded(&counter);
    // A turn that emptied everything leaves none, nor does a refused port.
    turn(&mut tally, &counter, &[], 0, 2, Publish::Chose);
    tally.yielded(&counter);
    turn(&mut tally, &counter, &[], 0, 0, Publish::Refused);
    tally.yielded(&counter);
    let read = counter.read();
    assert_eq!((read.yields, read.yields_with_work), (6, 4));
    assert_eq!(read.intake_full, 1);
}

#[test]
fn a_wait_counts_whether_it_found_something() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    tally.waited(&counter, false);
    tally.waited(&counter, false);
    tally.waited(&counter, true);
    let read = counter.read();
    assert_eq!((read.waits_pending, read.waits_ready), (2, 1));
    assert_eq!(read.turns, 0);
}

#[test]
fn an_ended_turn_starts_the_next_afresh() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    tally.intake_full();
    turn(&mut tally, &counter, &[], 1, 3, Publish::Budget);
    turn(&mut tally, &counter, &[], 0, 0, Publish::Chose);
    let read = counter.read();
    assert_eq!(read.accepted_by_turn, [1, 0, 0, 1, 0, 0, 0]);
    assert_eq!((read.intake_full, read.budget_turns), (1, 1));
}

#[test]
fn a_producer_s_steps_count_each_in_its_own_word() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    let steps = [
        (Local::Offered, 7),
        (Local::Refused, 1),
        (Local::Held, 2),
        (Local::Declined, 3),
        (Local::Withdrawn, 4),
        (Local::WaitsOffers, 5),
        (Local::WaitsChanges, 6),
    ];
    for (local, times) in steps {
        for _ in 0..times {
            tally.local(&counter, local);
        }
    }
    let read = counter.read();
    assert_eq!(
        [
            read.local_offered,
            read.local_refused,
            read.local_held,
            read.local_declined,
            read.local_withdrawn,
            read.local_waits_offers,
            read.local_waits_changes,
        ],
        [7, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!((read.turns, read.waits_pending), (0, 0), "nothing else");
}

#[test]
fn each_drop_counts_in_its_own_word_and_nothing_else() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    let reasons = [
        Dropped::Malformed,
        Dropped::Checksum,
        Dropped::NoSocket,
        Dropped::ReceiveFull,
        Dropped::NoBlock,
        Dropped::GroupLimit,
        Dropped::TooLong,
        Dropped::Fragment,
        Dropped::BadSource,
        Dropped::ReplyDropped,
        Dropped::SendOnly,
        Dropped::NoCredit,
        Dropped::NotLentStopping,
        Dropped::NotLentSocket,
        Dropped::PressureCopy,
        Dropped::LentLaneFull,
        Dropped::Dhcp,
        Dropped::Icmp,
    ];
    assert_eq!(reasons.len(), DROP_KINDS);
    for (times, reason) in (1..).zip(reasons) {
        for _ in 0..times {
            tally.dropped(&counter, reason);
        }
    }
    let read = counter.read();
    assert_eq!(read.drops, core::array::from_fn(|kind| kind as u32 + 1));
    assert_eq!(read.loans_at_turn_end, [0; LOAN_BINS]);
    assert_eq!(
        (read.turns, read.local_waits_changes),
        (0, 0),
        "nothing else"
    );
}

#[test]
fn a_turn_counts_the_loans_out_at_its_end_the_last_bin_holding_the_rest() {
    let counter = NetworkCounter::new();
    let mut tally = TurnTally::new();
    for loans in [0, 1, 1, 4, 5, 9] {
        tally.ended(&counter, true, true, loans);
    }
    let read = counter.read();
    assert_eq!(read.loans_at_turn_end, [1, 2, 0, 0, 1, 2]);
    assert_eq!(read.drops, [0; DROP_KINDS]);
}
