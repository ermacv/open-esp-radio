use super::*;

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}
use crate::state::RsnTxMessage;

fn transmit() -> RsnTransmit {
    RsnTransmit {
        message: RsnTxMessage::PairwiseMessage3,
        replay_counter: 8,
        retransmission: false,
    }
}

#[test]
fn each_alarm_edge_emits_at_most_one_bounded_retransmission() {
    let mut retry = RsnRetry::new(RsnRetryConfig {
        first_interval: Duration::from_micros(100_000),
        subsequent_interval: Duration::from_micros(1_000_000),
        attempts: 2,
    })
    .unwrap();
    let first = retry.arm(transmit(), at(10)).unwrap();
    assert_eq!(first.deadline, at(100_010));

    let RsnRetryAction::Transmit {
        frame,
        next_alarm: second,
    } = retry.on_alarm(first, first.deadline).unwrap()
    else {
        panic!("first alarm must retransmit and rearm")
    };
    assert!(frame.retransmission);
    assert_eq!(second.deadline, at(1_100_010));

    assert!(matches!(
        retry.on_alarm(second, second.deadline).unwrap(),
        RsnRetryAction::Transmit { .. }
    ));
    let exhausted = RsnRetryAlarm {
        generation: second.generation,
        deadline: second.deadline.checked_add(Duration::from_secs(1)).unwrap(),
    };
    assert_eq!(
        retry.on_alarm(exhausted, exhausted.deadline).unwrap(),
        RsnRetryAction::Exhausted
    );
    assert!(!retry.is_armed());
    assert_eq!(
        retry.on_alarm(exhausted, exhausted.deadline).unwrap(),
        RsnRetryAction::Stale
    );
}

#[test]
fn cancel_invalidates_an_already_programmed_alarm() {
    let mut retry = RsnRetry::new(RsnRetryConfig {
        first_interval: Duration::from_micros(1),
        subsequent_interval: Duration::from_micros(1),
        attempts: 1,
    })
    .unwrap();
    let alarm = retry.arm(transmit(), at(0)).unwrap();
    retry.cancel();
    assert_eq!(retry.on_alarm(alarm, at(1)).unwrap(), RsnRetryAction::Stale);
}

#[test]
fn acknowledged_initial_frame_uses_the_subsequent_response_window() {
    let mut retry = RsnRetry::new(RsnRetryConfig {
        first_interval: Duration::from_micros(100_000),
        subsequent_interval: Duration::from_micros(1_000_000),
        attempts: 3,
    })
    .unwrap();
    retry.arm(transmit(), at(10)).unwrap();
    assert_eq!(
        retry.defer_first_after_ack(at(20)).unwrap(),
        Some(RsnRetryAlarm {
            generation: 1,
            deadline: at(1_000_020),
        })
    );
}
