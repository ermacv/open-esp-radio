use super::*;

#[test]
fn blocked_socket_and_downstream_gap_are_distinct() {
    let socket = Counters::new();
    let radio = Counters::new();
    socket.admitted(0, 100);
    radio.admitted(0, 110);
    socket.blocked(120);
    socket.blocked(130);
    assert_eq!(socket.snapshot(140).pending_micros, 20);
    socket.admitted(1, 160);
    socket.admitted(2, 210);
    radio.admitted(1, 1_000);
    assert_eq!(socket.snapshot(220).maximum_wait_micros, 40);
    assert_eq!(socket.snapshot(220).maximum_gap_micros, 60);
    assert_eq!(radio.snapshot(1_010).maximum_gap_micros, 890);
    assert_eq!(radio.snapshot(1_010).sequence_after_maximum_gap, 1);
    assert_eq!(radio.snapshot(1_010).count, 2);
}

#[test]
fn clock_wrap_and_new_session_do_not_manufacture_a_gap() {
    let counter = Counters::new();
    counter.admitted(0, u32::MAX - 9);
    counter.admitted(1, 10);
    assert_eq!(counter.snapshot(20).maximum_gap_micros, 20);
    counter.reset(100);
    assert_eq!(counter.snapshot(100), Snapshot::default());
    counter.admitted(0, 200);
    assert_eq!(counter.snapshot(210).maximum_gap_micros, 0);
}

#[test]
fn stalled_frontier_retains_initial_and_trailing_silence() {
    let counters = Counters::new();
    counters.reset(100);
    let empty = counters.snapshot(6_000_100);
    assert_eq!(empty.count, 0);
    assert_eq!(empty.first_admission_micros, None);
    assert_eq!(empty.idle_micros, 6_000_000);
    counters.admitted(0, 6_000_100);
    let first = counters.snapshot(6_000_200);
    assert_eq!(first.first_admission_micros, Some(6_000_000));
    assert_eq!(first.idle_micros, 100);
    assert_eq!(first.maximum_gap_micros, 0);
}

#[test]
fn failed_send_closes_wait_without_inventing_a_packet() {
    let counters = Counters::new();
    counters.reset(100);
    counters.blocked(120);
    counters.failed(170);
    let failed = counters.snapshot(200);
    assert_eq!(failed.errors, 1);
    assert_eq!(failed.count, 0);
    assert_eq!(failed.maximum_wait_micros, 50);
    assert_eq!(failed.pending_micros, 0);
    counters.reset(300);
    assert_eq!(counters.snapshot(300), Snapshot::default());
}
