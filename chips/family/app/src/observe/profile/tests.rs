use super::*;

#[test]
fn observation_after_the_end_stamp_is_separate_from_body_cycles() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    turn(&mut tally, &counter, 100, 1000, [8, 0, 0, 0], None);
    let start = counter.read();
    counter.end_observed(300);
    turn(&mut tally, &counter, 1500, 2000, [8, 0, 0, 0], None);
    counter.end_observed(400);
    let window = TurnProfile::window(&start, &counter.read());
    assert_eq!(window.moment(0, 5), 2000);
    assert_eq!(window.end_observer_cycles, 700);
    assert_eq!(TurnProfile::from_words(&window.words()), window);
}

/// One turn from cycle `at` lasting `cycles`, with its items, and the pass
/// after it.
fn turn(
    tally: &mut ProfileTally,
    counter: &ProfileCounter,
    at: u64,
    cycles: u64,
    [frames, accepted, notices, requests]: [u32; 4],
    after: Option<Gap>,
) {
    tally.began(counter, at);
    for _ in 0..frames {
        tally.frame();
    }
    for _ in 0..accepted {
        tally.accepted();
    }
    for _ in 0..notices {
        tally.noticed();
    }
    for _ in 0..requests {
        tally.took();
    }
    tally.ended(counter, at + cycles);
    match after {
        Some(Gap::Yielded) => tally.yielded(),
        Some(Gap::Ready) => tally.waited(counter, true),
        Some(Gap::Pending) => tally.waited(counter, false),
        None => {}
    }
}

#[test]
fn the_moments_are_the_sums_of_every_product_of_a_turn_s_variables() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    let turns = [
        (1_000, [2, 0, 0, 1]),
        (3_000, [0, 4, 0, 0]),
        (500, [0, 0, 0, 0]),
        (7_000, [1, 2, 3, 4]),
    ];
    let mut at = 0;
    for (cycles, items) in turns {
        turn(&mut tally, &counter, at, cycles, items, Some(Gap::Yielded));
        at += cycles + 10;
    }
    let profile = counter.read();
    // The expected sums, computed apart from the counter.
    let variables: Vec<[u64; VARIABLES]> = turns
        .iter()
        .map(|(cycles, [f, a, n, r])| {
            [
                1,
                (*f).into(),
                (*a).into(),
                (*n).into(),
                (*r).into(),
                *cycles,
            ]
        })
        .collect();
    for i in 0..VARIABLES {
        for j in 0..VARIABLES {
            let expected: u64 = variables.iter().map(|v| v[i] * v[j]).sum();
            assert_eq!(profile.moment(i, j), expected, "moment ({i}, {j})");
        }
    }
    assert_eq!(profile.empty, [1, 500, 500 * 500], "the one empty turn");
    // Three gaps, each of 10 cycles after a yield.
    assert_eq!(profile.gaps[Gap::Yielded as usize], [3, 30]);
    assert_eq!(profile.gaps[Gap::Pending as usize], [0, 0]);
}

#[test]
fn cells_hold_turns_of_one_kind_of_work_only() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    turn(&mut tally, &counter, 0, 100, [3, 0, 0, 2], None);
    turn(&mut tally, &counter, 200, 400, [3, 0, 0, 0], None);
    turn(&mut tally, &counter, 1_000, 50, [0, 9, 0, 1], None);
    turn(&mut tally, &counter, 2_000, 70, [0, 0, 32, 0], None);
    // Mixed work counts in no cell.
    turn(&mut tally, &counter, 3_000, 90, [1, 1, 1, 0], None);
    let profile = counter.read();
    assert_eq!(
        profile.receive_only[2],
        [2, 500, 2],
        "two turns of three frames"
    );
    assert_eq!(
        profile.transmit_only[ITEM_CELLS - 1],
        [1, 50, 1],
        "nine accepted count with eight and more"
    );
    assert_eq!(profile.notice_only[5], [1, 70, 32], "17 to 32 notices");
    let celled: u64 = profile
        .receive_only
        .iter()
        .chain(&profile.transmit_only)
        .chain(&profile.notice_only)
        .map(|cell| cell[0])
        .sum();
    assert_eq!(celled, 4);
    assert_eq!(profile.moment(0, 0), 5);
}

#[test]
fn histograms_count_each_turn_once_and_settling_counts_its_publications() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    turn(&mut tally, &counter, 0, 300, [0, 0, 0, 0], None);
    turn(&mut tally, &counter, 1_000, 1 << 12, [8, 0, 2, 0], None);
    turn(&mut tally, &counter, 10_000, 1 << 24, [9, 3, 40, 0], None);
    let profile = counter.read();
    assert_eq!(profile.cycles_by_log2[0], 1, "below 2^9");
    assert_eq!(profile.cycles_by_log2[4], 1, "2^12");
    assert_eq!(profile.cycles_by_log2[CYCLE_BINS - 1], 1, "2^23 and more");
    assert_eq!(profile.notices_by_turn, [1, 0, 1, 0, 0, 0, 0, 1]);
    assert_eq!(profile.frames_by_turn, [1, 0, 0, 0, 0, 0, 0, 0, 2]);
    assert_eq!(
        profile.settling,
        [2, 1, 3],
        "two settling turns, one of them published three frames"
    );
}

#[test]
fn a_wait_marks_the_turn_before_it_idle_and_names_the_gap_after_it() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    turn(
        &mut tally,
        &counter,
        0,
        100,
        [0, 0, 0, 0],
        Some(Gap::Pending),
    );
    turn(
        &mut tally,
        &counter,
        5_100,
        200,
        [1, 0, 0, 0],
        Some(Gap::Yielded),
    );
    turn(
        &mut tally,
        &counter,
        5_400,
        50,
        [0, 0, 0, 0],
        Some(Gap::Ready),
    );
    turn(&mut tally, &counter, 5_460, 50, [0, 0, 0, 0], None);
    let profile = counter.read();
    assert_eq!(profile.idle, [2, 150], "the turns a wait followed");
    assert_eq!(profile.gaps[Gap::Pending as usize], [1, 5_000]);
    assert_eq!(profile.gaps[Gap::Yielded as usize], [1, 100]);
    assert_eq!(profile.gaps[Gap::Ready as usize], [1, 10]);
}

#[test]
fn a_turn_that_never_began_counts_nothing() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    tally.frame();
    tally.ended(&counter, 1_000);
    assert_eq!(counter.read(), TurnProfile::default());
}

#[test]
fn a_window_is_the_difference_of_two_readings_across_a_wrap() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    turn(&mut tally, &counter, 0, 100, [1, 0, 0, 0], None);
    let start = counter.read();
    turn(&mut tally, &counter, 1_000, 300, [2, 0, 0, 0], None);
    let window = TurnProfile::window(&start, &counter.read());
    assert_eq!(window.moment(0, 0), 1);
    assert_eq!(window.moment(1, 5), 600);
    assert_eq!(window.receive_only[1], [1, 300, 0]);
    let mut wrapped = TurnProfile::default();
    wrapped.moments[0] = u64::MAX;
    let mut after = TurnProfile::default();
    after.moments[0] = 4;
    assert_eq!(TurnProfile::window(&wrapped, &after).moments[0], 5);
    assert_eq!(TurnProfile::from_words(&window.words()), window);
}

#[test]
fn return_bursts_count_their_size_the_queue_around_them_and_what_stayed() {
    let counter = ReturnCounter::new();
    counter.burst(1, 0, 1, 0);
    counter.burst(16, 0, 16, 5);
    counter.burst(3, 13, 16, 0);
    let returns = counter.read();
    assert_eq!(returns.bursts, 3);
    assert_eq!(returns.queued, 20);
    assert_eq!(returns.queued_by_burst, [1, 0, 1, 0, 0, 1, 0]);
    assert_eq!(returns.before_by_burst, [2, 0, 0, 0, 1, 0]);
    assert_eq!(returns.after_by_burst, [0, 1, 0, 0, 0, 2]);
    assert_eq!((returns.held_bursts, returns.held), (1, 5));
    let start = returns;
    counter.burst(2, 0, 2, 0);
    let window = Returns::window(&start, &counter.read());
    assert_eq!(window.bursts, 1);
    assert_eq!(window.queued_by_burst, [0, 1, 0, 0, 0, 0, 0]);
    assert_eq!(window.held, 0);
}
