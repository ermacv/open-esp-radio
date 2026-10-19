use super::*;
use crate::wifi_status;

/// A scanning Wi-Fi beside BLE: the phases loop on the schedule's timer.
fn scanning_beside_ble() -> (CoexScheduleExecutor, CoexPhaseChange) {
    let mut executor = CoexScheduleExecutor::new();
    executor.set_interval(1024);
    assert_eq!(
        executor.set_status_bits(CoexStatusType::Wifi, wifi_status::SCAN),
        None
    );
    let change = executor
        .set_status_bits(CoexStatusType::Ble, 1)
        .expect("the first looping status restarts the phases");
    (executor, change)
}

fn generation(change: CoexPhaseChange) -> u32 {
    match change.timer {
        CoexPhaseTimer::Arm { generation, .. } => generation,
        CoexPhaseTimer::Disarm => panic!("the phase is timed"),
    }
}

#[test]
fn a_restart_arms_the_timer_for_the_phase_duration() {
    let (executor, change) = scanning_beside_ble();
    assert_eq!(executor.schedule().phase_index(), 0);
    assert!(executor.armed());
    assert_eq!(
        change.timer,
        CoexPhaseTimer::Arm {
            generation: generation(change),
            micros: change.step.timer_micros.unwrap(),
        }
    );
}

#[test]
fn an_expiry_of_the_armed_generation_steps_to_the_next_phase() {
    let (mut executor, change) = scanning_beside_ble();
    let CoexExpiry::Changed(next) = executor.expire(generation(change)) else {
        panic!("the armed expiry steps the schedule");
    };
    assert_eq!(executor.schedule().phase_index(), 1);
    assert_ne!(generation(next), generation(change));
}

#[test]
fn an_expiry_overtaken_by_a_later_phase_change_is_stale() {
    let (mut executor, first) = scanning_beside_ble();
    let restart = executor.restart();
    assert_eq!(executor.expire(generation(first)), CoexExpiry::Stale);
    assert_eq!(
        executor.schedule().phase_index(),
        0,
        "the stale expiry moved nothing"
    );
    assert!(matches!(
        executor.expire(generation(restart)),
        CoexExpiry::Changed(_)
    ));
}

#[test]
fn an_expiry_is_consumed_once() {
    let (mut executor, change) = scanning_beside_ble();
    let fired = generation(change);
    assert!(matches!(executor.expire(fired), CoexExpiry::Changed(_)));
    assert_eq!(executor.expire(fired), CoexExpiry::Stale);
}

#[test]
fn a_connected_wifi_leaves_the_timer_disarmed_at_the_last_phase() {
    let mut executor = CoexScheduleExecutor::new();
    executor.set_interval(1024);
    let _ = executor.set_status_bits(CoexStatusType::Wifi, wifi_status::CONNECTED);
    // A connected Wi-Fi does not restart on its own; it restarts per beacon.
    assert_eq!(executor.set_status_bits(CoexStatusType::Ble, 1), None);
    let mut change = executor.restart();
    while let CoexPhaseTimer::Arm { generation, .. } = change.timer {
        match executor.expire(generation) {
            CoexExpiry::Changed(next) => change = next,
            other => panic!("an armed phase steps: {other:?}"),
        }
    }
    assert!(!executor.armed());
    assert_eq!(
        executor.schedule().phase_index() + 1,
        executor.schedule().scheme().scheme().phases().len() as u8
    );
}

#[test]
fn clearing_status_keeps_the_armed_timer() {
    let (mut executor, change) = scanning_beside_ble();
    executor.clear_status_bits(CoexStatusType::Ble, 1);
    assert!(executor.armed());
    assert!(matches!(
        executor.expire(generation(change)),
        CoexExpiry::Changed(_) | CoexExpiry::Idle
    ));
}
