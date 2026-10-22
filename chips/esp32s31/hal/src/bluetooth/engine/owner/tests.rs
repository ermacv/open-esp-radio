use super::*;
use crate::bluetooth::engine::*;
use crate::dma_static::Placed;
use bytemuck::Zeroable;
use std::boxed::Box;

type Backing = BluetoothArena<0, 1, 2, 4, 4>;
type Memory = ArenaMemory<'static, 'static, 0, 1, 2, 4, 4, 8>;

fn clocked() -> bluetooth::ClockedOwner {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(());
    bluetooth::ClockedOwner::for_host_test(partitions.bluetooth)
}
fn config() -> SchedulerAllocationConfig {
    SchedulerAllocationConfig::new(1, 2, 0).unwrap()
}
fn placed() -> Placed<Backing> {
    Placed::host(Box::leak(Box::new(Backing::zeroed())))
}
fn model() -> (
    Engine<Memory>,
    bluetooth::InterruptOutputReleasedOwner,
    bluetooth::ModemLpTimerInterruptReadyOwner,
) {
    let (mut engine, interrupts) =
        Engine::for_host_test::<HostDma<0x2f00_1000>>(clocked(), placed(), config())
            .ok()
            .unwrap();
    let timer = bluetooth::ModemLpTimerInterruptReadyOwner {
        timer: engine.registers.modem_lp_timer.take().unwrap(),
    };
    let output = bluetooth::InterruptOutputReleasedOwner {
        registers: interrupts.registers,
    };
    (engine, output, timer)
}
fn reset(
    task: bluetooth::TaskOwner,
    output: bluetooth::InterruptOutputReleasedOwner,
    timer: bluetooth::ModemLpTimerInterruptReadyOwner,
) -> Result<
    (bluetooth::ClockedOwner, bluetooth::BluetoothControllerReset),
    bluetooth::BluetoothShutdownFailure,
> {
    assert!(!task.time_latch.in_flight());
    assert!(task.modem_lp_timer.is_none());
    Ok((
        bluetooth::ClockedOwner {
            partition: bluetooth::Partition {
                task: task.registers,
                modem_lp_timer: timer.timer,
                interrupts: output.registers,
            },
        },
        bluetooth::BluetoothControllerReset::for_host_test(),
    ))
}

#[test]
fn refusal_returns_the_exact_allocation_and_untouched_registers_for_retry() {
    let backing = Box::leak(Box::new(Backing::zeroed()));
    let address = core::ptr::from_ref(&*backing).addr();
    let result = Engine::<Memory>::for_host_test::<HostDma<0x2f00_1000>>(
        clocked(),
        Placed::host(backing),
        SchedulerAllocationConfig::new(0, 2, 0).unwrap(),
    );
    let Err((error, clocked, backing)) = result else {
        panic!("insufficient allocation numbers");
    };
    assert_eq!(error, ArenaError::AllocationNumbers);
    let backing = backing.into_inner();
    assert_eq!(core::ptr::from_ref(&*backing).addr(), address);
    let (task, interrupts) = clocked.separate_interrupt_owner();
    let clocked = task
        .into_clocked(interrupts)
        .expect("constructor refusal did not touch registers");
    let (engine, _) = Engine::<Memory>::for_host_test::<HostDma<0x2f00_1000>>(
        clocked,
        Placed::host(backing),
        config(),
    )
    .ok()
    .unwrap();
    assert!(engine.memory.devices.is_empty());
    assert_ne!(
        engine.memory.scanning_rx.initial_cursor(),
        engine.memory.other_rx.initial_cursor()
    );
}

#[test]
fn init_run_completion_stop_reset_returns_one_arena_and_rejects_old_items() {
    let (mut engine, output, timer) = model();
    let root = engine.memory.root_address_for_test();
    let epoch = engine.epoch;
    let hal_config = bluetooth::BluetoothControllerHalInitConfig::reviewed_standalone();
    engine
        .initialize_controller_hal_with(hal_config, |task, _| task.reunitable = false)
        .unwrap();
    assert_eq!(
        engine.initialize_controller_hal_with(hal_config, |_, _| panic!("repeated init")),
        Err(EngineInitError::AlreadyInitialized)
    );
    let lease = engine.dtm().unwrap().acquire().unwrap();
    engine
        .dtm()
        .unwrap()
        .prepare_event(&lease, |seed| {
            let words = seed.words();
            let item = words.scheduler_item().apply_event(
                DtmSchedulerFrequency::from_controller_image(2).unwrap(),
                DtmItemConfiguration::new(true, 0, None, SchedulerEventPriority::new(0).unwrap())
                    .unwrap(),
                100,
                150,
            );
            Ok::<_, core::convert::Infallible>(DtmPositionalEventWords::new(
                words.link_state().apply_reset(
                    Some(seed.tx_header_head_projection()),
                    Some(seed.rx_header_tail_projection()),
                    LeTxPower::from_dbm(0).unwrap(),
                    0,
                    false,
                ),
                item,
            ))
        })
        .unwrap();
    let id = engine.memory.dtm.submit(&lease, 0).unwrap();
    let mut items = SchedulerItemSpace::new().with(&engine.memory.dtm);
    let _insertion = engine
        .memory
        .scheduler
        .submit_idle(
            &mut items,
            &bluetooth::BluetoothSchedulerBusyObservation::from_busy_for_host_test(false),
            id,
            scheduler::SchedulerRawWindow::new(100, 150).unwrap(),
        )
        .unwrap();
    assert!(!engine.retirement_ready());
    assert!(
        engine.memory.dtm.release(lease).is_err(),
        "listed storage stays controller-owned"
    );
    let items = SchedulerItemSpace::new().with(&engine.memory.dtm);
    items.record_status_for_validation(id, 1);
    let raw = bluetooth::BluetoothSchedulerFinishedListObservation::from_lists_for_host_test(&[0])
        .unwrap();
    let observation = EngineFinishedListObservation::new(raw, engine.epoch);
    let EngineFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!()
    };
    let completion = engine.complete(observed).unwrap();
    assert_eq!(completion.iter().count(), 1);
    assert!(
        !engine.retirement_ready(),
        "empty list still needs a stop receipt"
    );
    engine.stopped_for_host_test();
    assert!(engine.retirement_ready());
    let raw = bluetooth::BluetoothSchedulerFinishedListObservation::from_lists_for_host_test(&[0])
        .unwrap();
    let EngineFinishedListPop::List {
        observed: stale, ..
    } = EngineFinishedListObservation::new(raw, engine.epoch).pop_lowest()
    else {
        panic!()
    };
    let returned = engine.shut_down_with(output, timer, reset).unwrap();
    let backing = returned.arena.into_inner();
    assert_eq!(core::ptr::from_ref(&*backing).addr(), root);
    let (mut next, _) = Engine::<Memory>::for_host_test::<HostDma<0x2f00_1000>>(
        returned.clocked,
        Placed::host(backing),
        config(),
    )
    .ok()
    .unwrap();
    assert_ne!(next.epoch, epoch);
    assert!(matches!(
        next.complete(stale),
        Err(CompletionError::ForeignEpoch(_))
    ));
    assert!(matches!(
        next.memory.dtm.retire(id),
        Err(SchedulerPoolError::StaleEpoch)
    ));
    assert!(next.dtm().unwrap().acquire().is_some());
}

#[test]
fn unconfirmed_stop_quarantines_the_exact_engine_without_resetting() {
    let (engine, output, timer) = model();
    let root = engine.memory.root_address_for_test();
    let error = engine
        .shut_down_with(output, timer, |_, _, _| panic!("reset before stop"))
        .err()
        .unwrap();
    assert_eq!(error.error(), EngineShutdownError::NotStopped);
    let ShutdownRetention::NotStopped { engine, .. } = &error._retained else {
        panic!()
    };
    assert_eq!(engine.memory.root_address_for_test(), root);
}

#[test]
fn reset_failure_is_absorbing_and_keeps_the_whole_arena() {
    let (mut engine, output, timer) = model();
    engine.stopped_for_host_test();
    let root = engine.memory.root_address_for_test();
    let error = engine
        .shut_down_with(output, timer, |task, output, timer| {
            Err(bluetooth::shutdown::failure_for_host_test(
                task,
                output.registers,
                timer.timer,
                bluetooth::BluetoothShutdownError::ControllerReset,
            ))
        })
        .err()
        .unwrap();
    assert_eq!(
        error.error(),
        EngineShutdownError::Reset(bluetooth::BluetoothShutdownError::ControllerReset)
    );
    let ShutdownRetention::Reset { memory, .. } = &error._retained else {
        panic!()
    };
    assert_eq!(memory.root_address_for_test(), root);
    assert!(!std::format!("{error:?}").contains("2f00"));
}

#[test]
fn uncertain_hardware_reachability_blocks_all_graph_access_even_after_late_stop() {
    let (mut engine, output, timer) = model();
    engine.faulted = true;
    engine.stopped_for_host_test();
    assert!(engine.is_quarantined());
    assert!(engine.dtm().is_err());
    assert!(engine.devices().is_err());
    assert!(!engine.retirement_ready());
    assert_eq!(
        engine
            .shut_down_with(output, timer, |_, _, _| panic!("quarantine cannot reset"))
            .err()
            .unwrap()
            .error(),
        EngineShutdownError::NotStopped
    );
}

#[test]
fn detached_completion_epochs_never_wrap_into_an_old_installation() {
    let counter = core::sync::atomic::AtomicU32::new(u32::MAX - 1);
    assert_eq!(
        super::super::arena::EngineEpoch::allocate(&counter),
        Some(super::super::arena::EngineEpoch(u32::MAX - 1))
    );
    assert_eq!(super::super::arena::EngineEpoch::allocate(&counter), None);
    assert_eq!(super::super::arena::EngineEpoch::allocate(&counter), None);
}
