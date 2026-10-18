use oer_esp32s31_hal::{bluetooth::ClockedOwner, root::RadioHardware};

use crate::controller_time::ControllerTimeWorkerPhase;

use super::separate_interrupt_owner;

fn clocked() -> ClockedOwner {
    let (_shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    ClockedOwner::for_validation(partitions.bluetooth)
}

#[test]
fn task_and_interrupt_owners_reunite_into_the_same_clocked_client() {
    let clocked = clocked();
    let (task, setup) = separate_interrupt_owner(clocked);
    assert_eq!(task.controller_time_retirement_ready(), Ok(()));
    assert_eq!(
        task.controller_time_phase(),
        ControllerTimeWorkerPhase::Idle
    );
    let _clocked = task
        .reunite(setup)
        .expect("untouched owners remain reunitable");
}

#[test]
fn task_retirement_keeps_a_controller_time_ownership_fault() {
    let clocked = clocked();
    let (mut task, setup) = separate_interrupt_owner(clocked);
    assert!(
        task.controller_time
            .cancel_owned(crate::controller_time::ControllerTimeRequest::for_validation(1))
            .is_err()
    );
    assert_eq!(
        task.controller_time_retirement_ready(),
        Err(crate::controller_time::ControllerTimeRetirementError::Faulted)
    );
    let _retained = (task, setup);
}
