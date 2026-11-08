use super::*;

#[test]
fn a_reading_keeps_each_tid_s_last_request_and_counts_every_outcome() {
    let counter = AddbaCounter::new();
    assert_eq!(counter.read(), Ok(Addba::default()));
    let accepted = Request {
        requested: 0,
        outcome: Outcome::Accepted { window: 16 },
    };
    let refused = Request {
        requested: 1023,
        outcome: Outcome::Refused { status: 37 },
    };
    let second = Request {
        requested: 64,
        outcome: Outcome::Accepted { window: 1 },
    };
    counter.record(0, refused);
    counter.record(0, accepted);
    counter.record(1, second);
    counter.record(
        15,
        Request {
            requested: 8,
            outcome: Outcome::Unanswered,
        },
    );
    counter.record(
        1,
        Request {
            requested: 64,
            outcome: Outcome::Repeated { window: 1 },
        },
    );
    // A TID no request field can name is counted, not kept.
    counter.record(16, accepted);
    let reading = counter.read().unwrap();
    assert_eq!(reading.last[0], Some(accepted));
    assert_eq!(
        reading.last[1],
        Some(Request {
            requested: 64,
            outcome: Outcome::Repeated { window: 1 },
        })
    );
    assert_eq!(
        reading.last[15],
        Some(Request {
            requested: 8,
            outcome: Outcome::Unanswered,
        })
    );
    assert!(reading.last[2..15].iter().all(Option::is_none));
    assert_eq!(
        (
            reading.accepted,
            reading.repeated,
            reading.refused,
            reading.unanswered
        ),
        (3, 1, 1, 1)
    );
}

#[test]
fn a_reading_during_a_write_is_refused() {
    let counter = AddbaCounter::new();
    counter.sequence.store(1, Ordering::Relaxed);
    assert_eq!(counter.read(), Err(Concurrent));
}
