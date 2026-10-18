use core::cell::Cell;

use embassy_futures::{block_on, join::join};

use super::{CONTINUE_BUDGET, ContinueBudget};

#[test]
fn a_runner_that_always_continues_lets_a_sibling_run() {
    let sibling_ran = Cell::new(false);
    let passes = Cell::new(0_u32);
    let runner = async {
        let mut budget = ContinueBudget::new();
        // An endless run of continued passes ends only once the sibling
        // on the same executor has made progress.
        while !sibling_ran.get() {
            passes.set(passes.get() + 1);
            budget.spend().await;
        }
    };
    let sibling = async { sibling_ran.set(true) };
    block_on(join(runner, sibling));
    assert!(sibling_ran.get());
    assert!(passes.get() <= u32::from(CONTINUE_BUDGET) + 1);
}

#[test]
fn awaiting_refills_the_budget() {
    block_on(async {
        let mut budget = ContinueBudget::new();
        for _ in 1..CONTINUE_BUDGET {
            budget.spend().await;
        }
        budget.refill();
        assert_eq!(budget.0, CONTINUE_BUDGET);
    });
}

/// The outcome queue reports a loss of received PDUs once, after the
/// outcomes queued before it and before those after it.
#[test]
fn a_received_pdu_loss_is_reported_in_its_place() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::{EventId, EventsLost};

    use super::OutcomeQueue;

    // Two slots for reports.
    let queue = OutcomeQueue::<NoopRawMutex, 2, 1>::new();
    let received = |byte| received(EventId::new(1), byte);
    for byte in 0..4 {
        queue.push(received(byte));
    }
    assert_eq!(queue.take(), Some(Ok(received(0))));
    assert_eq!(queue.take(), Some(Ok(received(1))));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(queue.take(), None);
    queue.push(received(5));
    assert_eq!(queue.take(), Some(Ok(received(5))));
}

/// An admitted event's end and the acknowledgement before it hold the
/// slots the admission reserved: received PDUs never take them.
#[test]
fn an_admitted_event_keeps_the_slots_of_its_terminal_outcomes() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::{ConnectionId, EventId, EventResult, EventsLost};

    use super::OutcomeQueue;
    use crate::BluetoothOutcome;

    let queue = OutcomeQueue::<NoopRawMutex, 3, 1>::new();
    assert!(queue.admit_event(EventId::new(1), 0));
    // One slot is left beside the two reserved ones.
    assert!(!queue.admit_event(EventId::new(2), 0));
    for byte in 0..3 {
        queue.push(received(EventId::new(1), byte));
    }
    let acknowledged = BluetoothOutcome::TransmitAcknowledged(ConnectionId::new(0));
    let ended = BluetoothOutcome::EventEnded {
        id: EventId::new(1),
        result: EventResult::NotExecuted,
    };
    queue.push(acknowledged.clone());
    queue.push(ended.clone());
    assert_eq!(queue.take(), Some(Ok(received(EventId::new(1), 0))));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(queue.take(), Some(Ok(acknowledged)));
    assert_eq!(queue.take(), Some(Ok(ended)));
    assert_eq!(queue.take(), None);
    // The event's slots are free again.
    assert!(queue.admit_event(EventId::new(2), 0));
}

/// A connection event reserves the data PDUs the hardware acknowledges to
/// the peer: they are never lost, while advertising reports of another
/// event are dropped when the rest of the queue is full.
#[test]
fn a_connection_event_keeps_the_slots_of_its_acknowledged_data() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::{EventId, EventResult, EventsLost};

    use super::OutcomeQueue;
    use crate::BluetoothOutcome;

    let queue = OutcomeQueue::<NoopRawMutex, 5, 1>::new();
    let connection = EventId::new(7);
    // Its end, the acknowledgement and two data PDUs: four of five slots.
    assert!(queue.admit_event(connection, 2));
    // A second connection event has no data slot left.
    assert!(!queue.admit_event(EventId::new(8), 0));
    // An advertising report fills the free slot; the next is dropped.
    queue.push(received(EventId::new(3), 0));
    queue.push(received(EventId::new(3), 1));
    // The connection's data still finds its reserved slots.
    queue.push(received(connection, 10));
    queue.push(received(connection, 11));
    let ended = BluetoothOutcome::EventEnded {
        id: connection,
        result: EventResult::NotExecuted,
    };
    queue.push(ended.clone());
    assert_eq!(queue.take(), Some(Ok(received(EventId::new(3), 0))));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(queue.take(), Some(Ok(received(connection, 10))));
    assert_eq!(queue.take(), Some(Ok(received(connection, 11))));
    assert_eq!(queue.take(), Some(Ok(ended)));
    // Every slot is free again, the unused companion slot included.
    assert!(!queue.admit_event(EventId::new(9), 4));
    assert!(queue.admit_event(EventId::new(9), 3));
}

/// A received PDU of `byte` during event `id`.
fn received(id: oer_bluetooth_radio::EventId, byte: u8) -> crate::BluetoothOutcome {
    crate::BluetoothOutcome::copy(oer_bluetooth_radio::RadioOutcome::Received {
        id,
        pdu: oer_bluetooth_radio::ReceivedPdu {
            pdu: &[0x02, 1, byte],
            rssi_dbm: -40,
            captured_at: None,
        },
    })
    .expect("the PDU fits")
}

#[test]
fn reports_take_only_the_slots_admitted_events_leave_free() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::EventId;

    use super::OutcomeQueue;

    let queue = OutcomeQueue::<NoopRawMutex, 4, 1>::new();
    // An admitted event's two slots, then reports until one is dropped.
    assert!(queue.admit_event(EventId::new(7), 0));
    queue.push(received(EventId::new(3), 0));
    queue.push(received(EventId::new(3), 1));
    queue.push(received(EventId::new(3), 2));
    assert_eq!(queue.entries.len(), 2);
    // Closing admission finds the event's end still owed.
    assert!(!queue.close());
}

#[test]
fn a_dropped_report_and_its_reported_loss_are_traced() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::{EventId, EventsLost};
    use oer_bluetooth_trace::{QueueLoss, QueueOp};

    use super::OutcomeQueue;

    let queue = OutcomeQueue::<NoopRawMutex, 2, 1>::new();
    for byte in 0..3 {
        queue.push(received(EventId::new(4), byte));
    }
    assert_eq!(
        crate::trace::take::<QueueLoss>(),
        [QueueLoss {
            op: QueueOp::ReportDropped,
            id: 4,
        }]
    );
    assert!(matches!(queue.take(), Some(Ok(_))));
    assert!(matches!(queue.take(), Some(Ok(_))));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(
        crate::trace::take::<QueueLoss>(),
        [QueueLoss {
            op: QueueOp::LossReported,
            id: 0,
        }]
    );
}
