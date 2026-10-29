use super::*;
use crate::bluetooth::engine::*;
use crate::memory::dma_static::Placed;
use bytemuck::Zeroable;
use std::boxed::Box;

type Backing = BluetoothArena<0, 1, 2, 1, 4, 4>;
type Memory = ArenaMemory<'static, 'static, 0, 1, 2, 1, 4, 4, 8>;

fn clocked() -> bluetooth::ClockedOwner {
    let (_, partitions) = crate::root::RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
            (),
            crate::domain::transaction::PhyIdentity::for_host_test(),
        )
        .expect("identity capacity");
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
    bluetooth::ModemLpTimerStoppedOwner,
) {
    let (mut engine, interrupts) =
        Engine::for_host_test::<HostDma<0x2f00_1000>>(clocked(), placed(), config())
            .ok()
            .unwrap();
    let timer = bluetooth::ModemLpTimerStoppedOwner {
        epoch: bluetooth::BluetoothModemLpTimerEpoch::new(),
        timer: engine.registers.modem_lp_timer.take().unwrap(),
    };
    let output = bluetooth::InterruptOutputReleasedOwner {
        registers: interrupts.registers,
    };
    (engine, output, timer)
}

fn prepared_dtm_item(
    engine: &mut Engine<Memory>,
) -> (SchedulerRoleInstance<'static, 'static>, SchedulerItemId) {
    let instance = engine.memory.dtm.acquire().expect("one DTM instance");
    engine
        .memory
        .dtm
        .prepare_event(&instance, |seed| {
            let words = seed.words();
            Ok::<_, core::convert::Infallible>(DtmPositionalEventWords::new(
                words.link_state().apply_reset(
                    Some(seed.tx_header_head_projection()),
                    Some(seed.rx_header_tail_projection()),
                    LeTxPower::from_dbm(0).expect("provider level"),
                    0,
                    false,
                ),
                words.scheduler_item(),
            ))
        })
        .expect("bound DTM image");
    let id = engine
        .memory
        .dtm
        .submit(&instance, 0)
        .expect("prepared item");
    (instance, id)
}

#[test]
fn dtm_busy_refusal_returns_the_original_graph_without_a_transaction() {
    let (mut engine, _, _) = model();
    let (instance, id) = prepared_dtm_item(&mut engine);
    assert!(engine.memory.dtm.is_listed(id));
    assert!(!engine.allow_busy_insertion::<false>(id).expect("same item"));
    assert!(!engine.memory.dtm.is_listed(id));
    let mut items = SchedulerItemSpace::new().with(&engine.memory.dtm);
    assert!(
        engine
            .memory
            .scheduler
            .take_completed(&mut items)
            .expect("no transaction was started")
            .is_empty()
    );
    // Nothing was published, so there is no hardware completion to decode.
    assert!(engine.memory.dtm.cpu(&instance).is_ok());
    assert!(engine.memory.dtm.release(instance).is_ok());
}

#[test]
fn shared_busy_insertion_keeps_the_item_for_the_hardware_transaction() {
    let (mut engine, _, _) = model();
    let (instance, id) = prepared_dtm_item(&mut engine);
    assert!(engine.allow_busy_insertion::<true>(id).expect("same item"));
    assert!(engine.memory.dtm.is_listed(id));
    let refused = engine
        .memory
        .dtm
        .release(instance)
        .expect_err("item stays listed");
    assert_eq!(refused.error, SchedulerPoolError::InstanceBusy);
    engine
        .return_item(id)
        .expect("no publication in this model");
    assert!(engine.memory.dtm.release(refused.instance).is_ok());
}

#[test]
fn reset_waits_for_both_quiet_interrupt_halves_and_the_channel_serves_the_next_epoch() {
    let (mut engine, output, timer) = model();
    let channel = Box::leak(Box::new(
        scheduler::interrupt::SchedulerInterruptChannel::new(),
    ));
    let half = engine
        .install_scheduler_interrupts(channel)
        .expect("fresh channel");
    let epoch = engine.epoch;
    let half = match engine.retire_scheduler_interrupts(half) {
        Err((scheduler::interrupt::SchedulerInterruptError::NotStopped, half)) => half,
        _ => panic!("a running scheduler retains both halves"),
    };
    engine.stopped_for_host_test();
    assert!(!engine.retirement_ready());
    assert!(engine.retire_scheduler_interrupts(half).is_ok());
    assert!(engine.retirement_ready());
    let returned = engine
        .shut_down_with(
            output,
            bluetooth::ModemLpTimerStopped::for_host_test(),
            reset,
        )
        .map(|pending| pending.reunite(timer))
        .expect("quiet reset");
    let (mut next, _) = Engine::<Memory>::for_host_test::<HostDma<0x2f00_1000>>(
        returned.clocked,
        returned.arena,
        config(),
    )
    .ok()
    .expect("reinstall returned allocation");
    assert_ne!(next.epoch, epoch);
    assert!(next.install_scheduler_interrupts(channel).is_ok());
}

#[test]
fn abandoning_an_exchange_quarantines_every_graph_publication() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    use scheduler::interrupt::{
        SchedulerDoorbell, SchedulerInterruptChannel, SchedulerInterruptError,
    };
    struct Bell;
    impl SchedulerDoorbell for Bell {
        fn ring(&self) {}
        fn poll_reply(&self, _: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>> {
            Poll::Pending
        }
    }
    let (mut engine, _, _) = model();
    let channel = Box::leak(Box::new(SchedulerInterruptChannel::new()));
    let half = engine
        .install_scheduler_interrupts(channel)
        .expect("fresh channel");
    {
        let task = engine
            .scheduler_interrupts
            .as_mut()
            .expect("installed task half");
        let mut future = core::pin::pin!(task.select_progress(&Bell));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert!(engine.is_quarantined());
    assert!(engine.dtm().is_err());
    assert!(engine.devices().is_err());
    assert_eq!(engine.publish_device_table(), Err(EngineFault));
    engine.stopped_for_host_test();
    assert!(!engine.retirement_ready());
    assert!(matches!(
        engine.retire_scheduler_interrupts(half),
        Err((SchedulerInterruptError::Quarantined, _))
    ));
    assert!(matches!(
        channel.install(engine.epoch.sequence()),
        Err(SchedulerInterruptError::Occupied)
    ));
}
fn reset(
    task: bluetooth::TaskOwner,
    output: bluetooth::InterruptOutputReleasedOwner,
    stopped: bluetooth::ModemLpTimerStopped,
) -> Result<bluetooth::BluetoothControllerResetTimerPending, bluetooth::BluetoothShutdownFailure> {
    assert!(!task.time_latch.in_flight());
    assert!(task.modem_lp_timer.is_none());
    Ok(bluetooth::shutdown::reset_for_host_test(
        task,
        output.registers,
        stopped,
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
        engine.memory.scanning_rx[0].initial_cursor(),
        engine.memory.other_rx[0].initial_cursor()
    );
}

#[test]
fn init_run_completion_stop_reset_returns_one_arena_and_rejects_old_items() {
    let (engine, output, timer) = model();
    epoch_cycle(engine, output, timer);
}

#[test]
fn dtm_only_profile_returns_one_arena_and_rejects_old_items() {
    type DtmMemory = ArenaMemory<'static, 'static, 0, 0, 0, 0, 0, 0, 1>;
    let backing = Box::leak(Box::new(BluetoothArena::<0, 0, 0, 0, 0, 0>::zeroed()));
    let (mut engine, interrupts) = Engine::<DtmMemory>::for_host_test::<HostDma<0x2f00_1000>>(
        clocked(),
        Placed::host(backing),
        config(),
    )
    .ok()
    .expect("DTM needs no global RX chains");
    let timer = bluetooth::ModemLpTimerStoppedOwner {
        epoch: bluetooth::BluetoothModemLpTimerEpoch::new(),
        timer: engine.registers.modem_lp_timer.take().unwrap(),
    };
    let output = bluetooth::InterruptOutputReleasedOwner {
        registers: interrupts.registers,
    };
    epoch_cycle(engine, output, timer);
}

fn epoch_cycle<
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
>(
    mut engine: Engine<ArenaMemory<'static, 'static, A, C, N, S, R, O, Q>>,
    output: bluetooth::InterruptOutputReleasedOwner,
    timer: bluetooth::ModemLpTimerStoppedOwner,
) {
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
    let returned = engine
        .shut_down_with(
            output,
            bluetooth::ModemLpTimerStopped::for_host_test(),
            reset,
        )
        .map(|pending| pending.reunite(timer))
        .unwrap();
    let backing = returned.arena.into_inner();
    assert_eq!(core::ptr::from_ref(&*backing).addr(), root);
    let (mut next, _) =
        Engine::<ArenaMemory<'static, 'static, A, C, N, S, R, O, Q>>::for_host_test::<
            HostDma<0x2f00_1000>,
        >(returned.clocked, Placed::host(backing), config())
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
    let (engine, output, _timer) = model();
    let root = engine.memory.root_address_for_test();
    let error = engine
        .shut_down_with(
            output,
            bluetooth::ModemLpTimerStopped::for_host_test(),
            |_, _, _| panic!("reset before stop"),
        )
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
    let (mut engine, output, _timer) = model();
    engine.stopped_for_host_test();
    let root = engine.memory.root_address_for_test();
    let error = engine
        .shut_down_with(
            output,
            bluetooth::ModemLpTimerStopped::for_host_test(),
            |task, output, stopped| {
                Err(bluetooth::shutdown::failure_for_host_test(
                    task,
                    output.registers,
                    bluetooth::BluetoothShutdownError::ControllerReset,
                    stopped,
                ))
            },
        )
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
    let (mut engine, output, _timer) = model();
    engine.faulted = true;
    engine.stopped_for_host_test();
    assert!(engine.is_quarantined());
    assert!(engine.dtm().is_err());
    assert!(engine.devices().is_err());
    assert!(!engine.retirement_ready());
    assert_eq!(
        engine
            .shut_down_with(
                output,
                bluetooth::ModemLpTimerStopped::for_host_test(),
                |_, _, _| panic!("quarantine cannot reset")
            )
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
        oer_espressif_bluetooth_engine::phase::EngineEpoch::allocate(&counter),
        Some(oer_espressif_bluetooth_engine::phase::EngineEpoch::from_sequence(u32::MAX - 1))
    );
    assert_eq!(
        oer_espressif_bluetooth_engine::phase::EngineEpoch::allocate(&counter),
        None
    );
    assert_eq!(
        oer_espressif_bluetooth_engine::phase::EngineEpoch::allocate(&counter),
        None
    );
}

#[test]
fn reset_returns_storage_outside_its_scoped_pool_and_epoch_brands() {
    let (clocked, arena) = with_pool(|pool| {
        with_epoch(|epoch| {
            let (mut engine, interrupts) =
                Engine::<ArenaMemory<'_, '_, 0, 1, 2, 1, 4, 4, 8>>::new::<HostDma<0x2f00_1000>>(
                    clocked(),
                    placed(),
                    pool,
                    epoch,
                    config(),
                )
                .ok()
                .unwrap();
            let timer = bluetooth::ModemLpTimerStoppedOwner {
                epoch: bluetooth::BluetoothModemLpTimerEpoch::new(),
                timer: engine.registers.modem_lp_timer.take().unwrap(),
            };
            let output = bluetooth::InterruptOutputReleasedOwner {
                registers: interrupts.registers,
            };
            engine.stopped_for_host_test();
            let returned = engine
                .shut_down_with(
                    output,
                    bluetooth::ModemLpTimerStopped::for_host_test(),
                    reset,
                )
                .map(|pending| pending.reunite(timer))
                .unwrap();
            (returned.clocked, returned.arena)
        })
    });
    with_pool(|pool| {
        with_epoch(|epoch| {
            let (mut engine, _) = Engine::<ArenaMemory<'_, '_, 0, 1, 2, 1, 4, 4, 8>>::new::<
                HostDma<0x2f00_1000>,
            >(clocked, arena, pool, epoch, config())
            .ok()
            .unwrap();
            assert!(engine.dtm().unwrap().acquire().is_some());
        })
    });
}

#[test]
fn shutdown_pending_reaches_the_reset_owner_and_the_reunited_caller() {
    let mut memory = bluetooth::registers::host::HostMemory::filled(0);
    memory.timer[0x24 / 4] = 7;
    memory.timer[0x2c / 4] = 3;
    let expected = memory.owners().2.stop_for_shutdown();
    assert_eq!((expected.state_0024(), expected.state_002c()), (7, 3));
    let stopped = bluetooth::ModemLpTimerStopped { pending: expected };
    let (mut engine, output, timer) = model();
    engine.stopped_for_host_test();
    let reset = engine
        .shut_down_with(output, stopped, |task, output, stopped| {
            Ok(bluetooth::shutdown::reset_for_host_test(
                task,
                output.registers,
                stopped,
            ))
        })
        .unwrap();
    assert_eq!(reset.pending_observation(), expected);
    assert_eq!(reset.reunite(timer).pending, expected);
}

#[test]
fn refused_shutdown_retains_the_sampled_pending_observation() {
    let mut memory = bluetooth::registers::host::HostMemory::filled(0);
    memory.timer[0x24 / 4] = 7;
    memory.timer[0x2c / 4] = 3;
    let expected = memory.owners().2.stop_for_shutdown();
    for stopped_engine in [false, true] {
        let (mut engine, output, _timer) = model();
        if stopped_engine {
            engine.stopped_for_host_test();
        }
        let failure = engine
            .shut_down_with(
                output,
                bluetooth::ModemLpTimerStopped { pending: expected },
                |task, output, stopped| {
                    Err(bluetooth::shutdown::failure_for_host_test(
                        task,
                        output.registers,
                        bluetooth::BluetoothShutdownError::ControllerReset,
                        stopped,
                    ))
                },
            )
            .err()
            .unwrap();
        assert_eq!(failure.pending_observation(), expected);
        assert_eq!(
            failure.error(),
            if stopped_engine {
                EngineShutdownError::Reset(bluetooth::BluetoothShutdownError::ControllerReset)
            } else {
                EngineShutdownError::NotStopped
            }
        );
    }
}
