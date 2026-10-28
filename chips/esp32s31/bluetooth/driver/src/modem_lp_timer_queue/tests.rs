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
    let cell = ModemLpTimerWorkerWakeCell::new(
        oer_espressif_interrupt_table::signal::host::deferred_signal().0,
    );

    assert_eq!(
        ModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
        ModemLpTimerPublishedInterruptStep::SoftwarePending(
            ModemLpTimerWorkerWakePublication::Pended,
        )
    );
    assert_eq!(cell.is_pending(), Ok(true));
    assert_eq!(
        ModemLpTimerStableInterruptStep::AwaitingSoftware.publish(&cell),
        ModemLpTimerPublishedInterruptStep::AwaitingSoftware(
            ModemLpTimerWorkerWakePublication::Coalesced,
        )
    );

    assert_eq!(cell.take(), Ok(true));
    assert_eq!(cell.is_pending(), Ok(false));
    assert_eq!(
        ModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
        ModemLpTimerPublishedInterruptStep::SoftwarePending(
            ModemLpTimerWorkerWakePublication::Pended,
        )
    );
}

#[test]
fn source_127_readiness_wakes_the_worker_without_being_taken() {
    use core::task::{Context, Poll, Waker};

    let cell =
        ModemLpTimerWorkerWakeCell::new(oer_espressif_interrupt_table::signal::host::signal().0);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(cell.poll_pending(&mut cx), Poll::Pending);
    let _ = ModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell);
    assert_eq!(cell.poll_pending(&mut cx), Poll::Ready(Ok(())));
    // Readiness stays until the worker acquires the owner and takes it.
    assert_eq!(cell.is_pending(), Ok(true));
    assert_eq!(cell.take(), Ok(true));
    assert_eq!(cell.poll_pending(&mut cx), Poll::Pending);

    let foreign = ModemLpTimerWorkerWakeCell::new(
        oer_espressif_interrupt_table::signal::host::foreign_signal(),
    );
    let _ = ModemLpTimerStableInterruptStep::SoftwarePending.publish(&foreign);
    assert_eq!(
        foreign.poll_pending(&mut cx),
        Poll::Ready(Err(oer_espressif_interrupt_table::signal::ForeignContext))
    );
}

#[test]
fn p2_uses_one_snapshot_and_returns_a_plan_after_publication() {
    use oer_time::{Clock, Duration, Instant};
    use oer_time_virtual::VirtualClock;

    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(24));
    let mut queue = ModemLpTimerQueue::<3>::new();
    let _first = queue.schedule(instant(10), instant(20)).unwrap();
    let _second = queue.schedule(instant(10), instant(25)).unwrap();
    let _last = queue.schedule(instant(10), instant(30)).unwrap();
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    let snapshot =
        ModemLpTimerWork::for_host_test(instant(clock.now().as_micros() as u32), epoch, true);
    let work = ModemLpTimerSoftwareWork::begin(&mut queue, &mut epoch, snapshot);
    let ModemLpTimerSoftwareStep::Expiration(pending) = work.step() else {
        panic!("the oldest due expiration must be published first");
    };
    assert_eq!(pending.event().deadline(), instant(20));
    let events = ModemLpTimerEventCell::new();
    let (work, _) = pending
        .publish::<oer_espressif_interrupt_table::signal::host::Deferred>(&events)
        .ok()
        .unwrap();
    clock.advance(Duration::from_micros(20)).unwrap();
    // P2 has only the handler's old snapshot, even when it is delayed.
    let ModemLpTimerSoftwareStep::Planned(plan) = work.step() else {
        panic!("new time must be reconciled by the handler");
    };
    assert_eq!(plan.deadline(), Some(instant(25)));
    assert_eq!(events.take().unwrap().deadline(), instant(20));
    assert_eq!(queue.next_deadline(instant(44)), Some(instant(25)));
}

#[test]
fn rollover_only_snapshot_plans_without_dispatching_and_empty_queue_stays_unarmed() {
    let mut queue = ModemLpTimerQueue::<1>::new();
    let _timer = queue.schedule(instant(10), instant(20)).unwrap();
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    let snapshot = ModemLpTimerWork::for_host_test(instant(25), epoch, false);
    let work = ModemLpTimerSoftwareWork::begin(&mut queue, &mut epoch, snapshot);
    let ModemLpTimerSoftwareStep::Planned(plan) = work.step() else {
        panic!("rollover alone does not dispatch the queue");
    };
    assert_eq!(plan.deadline(), Some(instant(20)));

    let mut empty = ModemLpTimerQueue::<0>::new();
    let snapshot = ModemLpTimerWork::for_host_test(instant(25), epoch, true);
    let work = ModemLpTimerSoftwareWork::begin(&mut empty, &mut epoch, snapshot);
    let ModemLpTimerSoftwareStep::Planned(plan) = work.step() else {
        panic!("an empty queue returns an empty plan");
    };
    assert_eq!(plan.deadline(), None);
}
