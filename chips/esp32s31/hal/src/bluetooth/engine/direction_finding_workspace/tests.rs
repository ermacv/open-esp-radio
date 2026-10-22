use super::DirectionFindingWorkspaceStorage;
use bytemuck::Zeroable;

fn storage() -> &'static mut DirectionFindingWorkspaceStorage {
    std::boxed::Box::leak(std::boxed::Box::new(
        DirectionFindingWorkspaceStorage::zeroed(),
    ))
}

#[test]
fn placing_zeroed_storage_initializes_the_disabled_cte_workspace() {
    let owner = DirectionFindingWorkspaceStorage::place::<
        crate::bluetooth::engine::HostDma<0x2f00_1000>,
    >(crate::dma_static::Placed::host(storage()));

    assert!(owner.is_disabled_baseline_initialized());
}

#[test]
fn a_controller_reset_restores_the_disabled_cte_workspace() {
    let mut owner = DirectionFindingWorkspaceStorage::place::<
        crate::bluetooth::engine::HostDma<0x2f00_1000>,
    >(crate::dma_static::Placed::host(storage()));
    // Hardware state the finished epoch left behind.
    owner.emulate_hardware_clear();
    assert!(!owner.is_disabled_baseline_initialized());

    owner
        .reset_after_controller_reset(&crate::bluetooth::BluetoothControllerReset::for_host_test());

    assert!(owner.is_disabled_baseline_initialized());
}
