use super::{
    DirectionFindingWorkspaceBindError, DirectionFindingWorkspaceModelAddress,
    DirectionFindingWorkspaceStorage,
};

fn storage() -> &'static mut DirectionFindingWorkspaceStorage {
    std::boxed::Box::leak(std::boxed::Box::new(DirectionFindingWorkspaceStorage::new()))
}

#[test]
fn model_binding_initializes_the_disabled_cte_workspace() {
    let base =
        DirectionFindingWorkspaceModelAddress::new(0x2f00_1000).expect("model base is encodable");
    let owner = DirectionFindingWorkspaceStorage::pin_static_model(storage(), base)
        .expect("the complete workspace fits physical SRAM");

    assert!(owner.is_disabled_baseline_initialized());
}

#[test]
fn model_binding_rejects_a_crossing_extent() {
    let base = DirectionFindingWorkspaceModelAddress::new(0x2f07_fff0)
        .expect("crossing base itself is encodable");
    let failure = match DirectionFindingWorkspaceStorage::pin_static_model(storage(), base) {
        Ok(_) => panic!("the complete workspace must fit physical SRAM"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.error(),
        DirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram
    );
}

#[test]
fn a_controller_reset_restores_the_disabled_cte_workspace() {
    let base =
        DirectionFindingWorkspaceModelAddress::new(0x2f00_1000).expect("model base is encodable");
    let mut owner = DirectionFindingWorkspaceStorage::pin_static_model(storage(), base)
        .expect("the complete workspace fits physical SRAM");
    // Hardware state the finished epoch left behind.
    owner.emulate_hardware_clear();
    assert!(!owner.is_disabled_baseline_initialized());

    owner.reset_after_controller_reset(
        &oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation(),
    );

    assert!(owner.is_disabled_baseline_initialized());
}
