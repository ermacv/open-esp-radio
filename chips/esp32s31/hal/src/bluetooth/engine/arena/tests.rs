use super::*;
use crate::bluetooth::ClockedOwner;
use bytemuck::Zeroable;
use std::boxed::Box;

#[test]
fn uninitialized_registers_cannot_mint_a_memory_engine_capability() {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(());
    let (mut task, _irq) =
        ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
    assert!(task.reunitable);
    assert!(matches!(
        task.claim_engine_registers(),
        Err(ArenaError::NotInitialized)
    ));
    assert!(
        task.reunitable,
        "rejection does not consume the untouched register state"
    );
}

#[test]
fn one_arena_retains_distinct_fields_in_the_root_address_space() {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(());
    let (mut task, _irq) =
        ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
    task.arena_state = crate::bluetooth::ControllerArenaState::Ready;
    let arena = Box::leak(Box::new(BluetoothArena::<1, 1, 1, 2, 2>::zeroed()));
    let config = SchedulerAllocationConfig::new(2, 1, 0).expect("test capacities fit");
    with_pool(|pool| {
        with_epoch(|epoch| {
            let mut engine = Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(
                task.claim_engine_registers().expect("HAL initialized"),
                Placed::host(arena),
                pool,
                epoch,
                config,
            )
            .ok()
            .expect("the arena fits");
            let dtm = engine.dtm.acquire().expect("DTM slot");
            let adv = engine.advertising.acquire().expect("advertising slot");
            let dtm_space = SchedulerItemSpace::new().with(&engine.dtm);
            let adv_space = SchedulerItemSpace::new().with(&engine.advertising);
            assert_ne!(dtm_space.link(dtm.item(0)), adv_space.link(adv.item(0)));
            assert_ne!(
                engine.scanning_rx.initial_cursor(),
                engine.other_rx.initial_cursor()
            );
            assert!(engine.direction_finding.is_disabled_baseline_initialized());
            assert!(engine.devices.is_empty());
            engine.dtm.release(dtm).expect("unused graph returns");
            engine
                .advertising
                .release(adv)
                .expect("unused graph returns");
        })
    });
}

#[test]
fn an_invalid_arena_returns_both_owners_without_claiming_any_graph() {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(());
    let (mut task, _irq) =
        ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
    task.arena_state = crate::bluetooth::ControllerArenaState::Ready;
    let arena = Box::leak(Box::new(BluetoothArena::<1, 1, 1, 2, 2>::zeroed()));
    with_pool(|pool| {
        with_epoch(|epoch| {
            let result = Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(
                task.claim_engine_registers().expect("HAL initialized"),
                Placed::host(arena),
                pool,
                epoch,
                SchedulerAllocationConfig::new(1, 1, 0)
                    .expect("valid but insufficient advertising numbers"),
            );
            let Err((error, controller, placed)) = result else {
                panic!("insufficient numbers must be refused");
            };
            assert_eq!(error, ArenaError::AllocationNumbers);
            // Rejection leaves placement and registers available for a corrected request.
            with_pool(|pool| {
                with_epoch(|epoch| {
                    assert!(
                        Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(
                            controller,
                            placed,
                            pool,
                            epoch,
                            SchedulerAllocationConfig::new(2, 1, 0).expect("corrected capacities"),
                        )
                        .is_ok()
                    );
                })
            });
        })
    });
}

#[test]
fn dropping_an_engine_does_not_admit_a_second_arena_in_the_same_epoch() {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(());
    let (mut task, _irq) =
        ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
    task.arena_state = crate::bluetooth::ControllerArenaState::Ready;
    let first = Box::leak(Box::new(BluetoothArena::<1, 1, 1, 2, 2>::zeroed()));
    let second = Box::leak(Box::new(BluetoothArena::<1, 1, 1, 2, 2>::zeroed()));
    let config = SchedulerAllocationConfig::new(2, 1, 0).expect("test capacities fit");
    with_pool(|pool| {
        with_epoch(|epoch| {
            let engine = Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(
                task.claim_engine_registers().expect("HAL initialized"),
                Placed::host(first),
                pool,
                epoch,
                config,
            )
            .ok()
            .expect("first installation");
            drop(engine);
        })
    });
    with_pool(|pool| {
        with_epoch(|epoch| {
            let result = Engine::<1, 1, 1, 2, 2, 4>::bind::<HostDma<0x2f00_8000>>(
                &mut task,
                Placed::host(second),
                pool,
                epoch,
                config,
            );
            let Err(error) = result else {
                panic!("epoch already retains an arena");
            };
            assert_eq!(error.error(), ArenaError::AlreadyInstalled);
        })
    });
}
