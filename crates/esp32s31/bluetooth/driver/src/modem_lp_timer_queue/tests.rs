use super::*;

fn instant(bits: u32) -> BluetoothModemLpTimerInstant {
    BluetoothModemLpTimerInstant::from_bits(bits)
}

#[test]
fn queue_expires_oldest_due_timer_one_step_at_a_time() {
    let mut queue = ModemLpTimerQueue::<3>::new();
    let _later = queue.schedule(instant(10), instant(30)).unwrap();
    let _first = queue.schedule(instant(10), instant(20)).unwrap();
    let _middle = queue.schedule(instant(10), instant(25)).unwrap();

    assert_eq!(queue.pop_due(instant(24)).unwrap().deadline(), instant(20));
    assert!(queue.pop_due(instant(24)).is_none());
    assert_eq!(queue.next_deadline(instant(24)), Some(instant(25)));
}

#[test]
fn rearm_selection_prefers_an_overdue_timer_to_a_future_timer() {
    let mut queue = ModemLpTimerQueue::<2>::new();
    let _overdue = queue.schedule(instant(10), instant(20)).unwrap();
    let _future = queue.schedule(instant(10), instant(40)).unwrap();

    assert_eq!(queue.next_deadline(instant(30)), Some(instant(20)));
}

#[test]
fn cancellation_generation_cannot_target_a_reused_slot() {
    let mut queue = ModemLpTimerQueue::<1>::new();
    let old = queue.schedule(instant(0), instant(1)).unwrap();
    let expired = queue.pop_due(instant(1)).unwrap();
    let current = queue.schedule(instant(1), instant(2)).unwrap();

    assert_ne!(expired.generation(), current.generation);
    assert!(!queue.cancel(old));
    assert!(queue.cancel(current));
    assert!(queue.is_empty());
}

#[test]
fn wrapping_deadlines_are_ordered_within_the_forward_half_range() {
    let mut queue = ModemLpTimerQueue::<2>::new();
    let _first = queue.schedule(instant(u32::MAX - 2), instant(1)).unwrap();
    let _second = queue.schedule(instant(u32::MAX - 2), instant(4)).unwrap();

    assert_eq!(queue.pop_due(instant(2)).unwrap().deadline(), instant(1));
    assert_eq!(queue.next_deadline(instant(2)), Some(instant(4)));
    assert!(matches!(
        queue.schedule(instant(2), instant(1)),
        Err(ModemLpTimerScheduleError::DeadlineOutsideForwardHalfRange)
    ));
}

#[test]
fn full_event_cell_applies_backpressure_without_overwrite() {
    let cell = ModemLpTimerEventCell::new();
    let first = ModemLpTimerExpiration {
        slot: 1,
        generation: 7,
        deadline: instant(100),
    };
    let second = ModemLpTimerExpiration {
        slot: 2,
        generation: 9,
        deadline: instant(200),
    };

    assert_eq!(
        cell.publish(first),
        Ok(ModemLpTimerEventPublication::WakeWorker)
    );
    assert_eq!(cell.publish(second), Err(second));
    assert_eq!(cell.take(), Some(first));
    assert_eq!(
        cell.publish(second),
        Ok(ModemLpTimerEventPublication::WakeWorker)
    );
    assert_eq!(cell.take(), Some(second));
}

#[test]
fn source_127_task_readiness_survives_late_acquisition_and_coalesces_reentry() {
    let cell = ModemLpTimerWorkerWakeCell::new();

    assert_eq!(
        ModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
        ModemLpTimerPublishedInterruptStep::SoftwarePending(
            ModemLpTimerWorkerWakePublication::WakeWorker,
        )
    );
    assert!(cell.is_pending());
    assert_eq!(
        ModemLpTimerStableInterruptStep::AwaitingSoftware.publish(&cell),
        ModemLpTimerPublishedInterruptStep::AwaitingSoftware(
            ModemLpTimerWorkerWakePublication::Coalesced,
        )
    );

    assert!(cell.take());
    assert!(!cell.is_pending());
    assert_eq!(
        ModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
        ModemLpTimerPublishedInterruptStep::SoftwarePending(
            ModemLpTimerWorkerWakePublication::WakeWorker,
        )
    );
}
