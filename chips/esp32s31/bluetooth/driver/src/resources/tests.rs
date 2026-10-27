use oer_esp32s31_hal::{bluetooth::ClockedOwner, root::RadioHardware};

use crate::controller_time::ControllerTimeWorkerPhase;

use super::separate_interrupt_owner;

fn clocked() -> ClockedOwner {
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        );
    ClockedOwner::for_host_test(partitions.bluetooth)
}

#[test]
fn task_and_interrupt_owners_reunite_into_the_same_clocked_client() {
    let clocked = clocked();
    let (engine, interrupts) = crate::test_clocks::engine(clocked);
    let (task, setup) = separate_interrupt_owner(engine, interrupts);
    assert_eq!(task.controller_time_retirement_ready(), Ok(()));
    assert_eq!(
        task.controller_time_phase(),
        ControllerTimeWorkerPhase::Idle
    );
    assert!(
        !task.engine_retirement_ready(),
        "retirement requires the engine's stop receipt"
    );
    let _retained = (task, setup);
}

#[test]
fn task_retirement_keeps_a_controller_time_ownership_fault() {
    let clocked = clocked();
    let (engine, interrupts) = crate::test_clocks::engine(clocked);
    let (mut task, setup) = separate_interrupt_owner(engine, interrupts);
    assert!(
        task.controller_time
            .cancel_owned(crate::controller_time::ControllerTimeRequest::for_host_test(1))
            .is_err()
    );
    assert_eq!(
        task.controller_time_retirement_ready(),
        Err(crate::controller_time::ControllerTimeRetirementError::Faulted)
    );
    let _retained = (task, setup);
}
