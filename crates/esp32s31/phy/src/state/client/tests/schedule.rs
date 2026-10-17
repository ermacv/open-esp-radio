use super::*;
use crate::tracking::schedule::Schedule;

#[test]
fn deferring_demand_never_refreshes_the_source_scheduler() {
    let state = state_for_mask(WIFI_BIT | BLUETOOTH_BIT, 0);
    let before = state.snapshot();
    let due_at = before.next_tracking_deadline_micros().unwrap().unwrap();
    assert_eq!(
        before.tracking_schedule_at(due_at - 1),
        Ok(Schedule::At(due_at))
    );
    let Schedule::Due(first) = before.tracking_schedule_at(due_at).unwrap() else {
        panic!("first due instant must request work");
    };
    let deferred_until = due_at + 3 * DEFAULT_PLL_TRACK_PERIOD_MICROS;
    assert_eq!(
        state.snapshot().tracking_schedule_at(deferred_until),
        Ok(Schedule::Due(first))
    );
    assert_eq!(first.due_since_micros(), due_at);
    assert_eq!(state.snapshot(), before);

    // Only the consuming execution request refreshes the source timestamps.
    let started = state.evaluate_immediate_at(deferred_until).unwrap();
    assert_eq!(started.request().copied(), Some(first.request()));
    assert_eq!(
        started
            .owner()
            .snapshot()
            .previous_micros(PhyPllTrackClass::Wifi),
        deferred_until
    );
    assert!(
        started.into_owner().is_err(),
        "request is not a completed calibration"
    );
}

#[test]
fn current_client_set_replaces_stale_demand_after_a_deferred_release() {
    let state = state_for_mask(VALID_CLIENT_BITS, 0);
    let at = DEFAULT_PLL_TRACK_PERIOD_MICROS + 1;
    let Schedule::Due(old) = state.snapshot().tracking_schedule_at(at).unwrap() else {
        panic!("all clients are due");
    };
    assert!(old.request().wifi());
    assert!(old.request().bluetooth_ieee802154());

    let state = state.release(RadioClient::Wifi).unwrap().owner;
    let state = state.release(RadioClient::Bluetooth).unwrap().owner;
    let Schedule::Due(current) = state.snapshot().tracking_schedule_at(at).unwrap() else {
        panic!("IEEE client still requires its shared tracking class");
    };
    assert!(!current.request().wifi());
    assert!(current.request().bluetooth_ieee802154());
    let state = state.release(RadioClient::Ieee802154).unwrap().owner;
    assert_eq!(
        state.snapshot().tracking_schedule_at(u64::MAX),
        Ok(Schedule::Inactive)
    );
}

#[test]
fn clock_reversal_cannot_hide_as_not_yet_due() {
    let now = 2 * DEFAULT_PLL_TRACK_PERIOD_MICROS;
    let state = state_for_mask(WIFI_BIT, now);
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.tracking_schedule_at(now - 1),
        Err(PhyTrackTimeError::TimeReversed {
            class: PhyPllTrackClass::Wifi,
            previous_micros: now,
            now_micros: now - 1,
        })
    );
    assert_eq!(state.snapshot(), snapshot);
    assert!(matches!(
        snapshot.tracking_schedule_at(now),
        Ok(Schedule::At(_))
    ));
}

#[test]
fn impossible_deadline_is_an_error_without_losing_the_owner() {
    let state = state_for_mask(IEEE802154_BIT, u64::MAX);
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.tracking_schedule_at(u64::MAX),
        Err(PhyTrackTimeError::DeadlineOverflow {
            class: PhyPllTrackClass::BluetoothIeee802154,
        })
    );
    assert_eq!(state.snapshot(), snapshot);
    let state = state.release(RadioClient::Ieee802154).unwrap().owner;
    assert_eq!(
        state.snapshot().tracking_schedule_at(0),
        Ok(Schedule::Inactive)
    );
}
