use super::{
    BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, service_stable_owner, validate_interrupt_storage,
};

#[test]
fn interrupt_retirement_preserves_rejected_owner_and_never_reopens_its_epoch() {
    use super::{
        EspHalBluetoothInterruptRetirementError as Error, InterruptStorageReservation,
        StoredModemTimerOwner, retire_interrupt_owner, retire_ready_timer,
    };
    let mut reservation = InterruptStorageReservation::new();
    reservation.claim(false, false).unwrap();
    let mut interrupts = Some(std::boxed::Box::new(17));
    let identity = core::ptr::from_ref(&**interrupts.as_ref().unwrap());
    let mut timer: Option<StoredModemTimerOwner<_, ()>> =
        Some(StoredModemTimerOwner::Ready(std::boxed::Box::new(29)));
    for (routes_bound, timer_present, expected) in [
        (true, true, Error::RoutesBound),
        (false, true, Error::TimerStillPublished),
        (true, false, Error::RoutesBound),
    ] {
        assert_eq!(
            retire_interrupt_owner(&mut interrupts, routes_bound, timer_present),
            Err(expected)
        );
        assert_eq!(
            core::ptr::from_ref(&**interrupts.as_ref().unwrap()),
            identity
        );
    }
    assert_eq!(
        service_stable_owner(&mut interrupts, |owner| {
            **owner += 1;
            **owner
        }),
        Some(18)
    );
    let timer = retire_ready_timer(&mut timer, false).unwrap();
    let retired = retire_interrupt_owner(&mut interrupts, false, false).unwrap();
    assert_eq!(core::ptr::from_ref(&*retired), identity);
    assert_eq!(*retired, 18);
    assert_eq!(*timer, 29);
    assert_eq!(
        retire_interrupt_owner(&mut interrupts, false, false),
        Err(Error::Missing)
    );
    assert_eq!(
        service_stable_owner(&mut interrupts, |_| panic!(
            "retired slot cannot service IRQ"
        )),
        None::<()>
    );
    // Both real slots are empty, but old Controller references still exist.
    assert_eq!(
        reservation.claim(false, false),
        Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
    );
    drop((retired, timer));
    assert_eq!(
        reservation.claim(false, false),
        Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
    );
}

#[test]
fn failed_initial_publication_does_not_claim_a_reservation() {
    use super::InterruptStorageReservation;
    let mut reservation = InterruptStorageReservation::new();
    assert_eq!(
        reservation.claim(true, false),
        Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
    );
    reservation.claim(false, false).unwrap();
    assert_eq!(
        reservation.claim(true, false),
        Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
    );
}

#[test]
fn route_epoch_binds_once_and_dispatches_only_while_live() {
    let mut state = BluetoothInterruptRouteState::from_bound_core(None);
    assert!(!state.dispatch_is_live());
    assert_eq!(state.bind(0_u8), Ok(()));
    assert!(state.dispatch_is_live());
    assert_eq!(
        state.bind(0),
        Err(EspHalBluetoothInterruptRouteError::AlreadyBound)
    );
}

#[test]
fn route_epoch_disable_is_same_core_and_consumes_live_dispatch() {
    let mut state = BluetoothInterruptRouteState::inactive();
    assert_eq!(state.bind(1_u8), Ok(()));
    assert_eq!(
        state.disable(0),
        Err(EspHalBluetoothInterruptRouteError::WrongCore)
    );
    assert!(state.dispatch_is_live());
    assert_eq!(state.disable(1), Ok(()));
    assert!(!state.dispatch_is_live());
    assert_eq!(
        state.disable(1),
        Err(EspHalBluetoothInterruptRouteError::Inactive)
    );
}

#[test]
fn both_isr_owner_slots_publish_or_reject_as_one_invariant() {
    assert_eq!(validate_interrupt_storage(false, false), Ok(()));
    assert_eq!(
        validate_interrupt_storage(true, true),
        Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
    );
    assert_eq!(
        validate_interrupt_storage(true, false),
        Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
    );
    assert_eq!(
        validate_interrupt_storage(false, true),
        Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
    );
}

#[test]
fn source_127_register_entry_stops_while_software_owns_the_timer() {
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Missing),
        BluetoothModemLpTimerInterruptAdmission::Unavailable
    );
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Ready),
        BluetoothModemLpTimerInterruptAdmission::ServiceRegisters
    );
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::SoftwarePending),
        BluetoothModemLpTimerInterruptAdmission::AwaitSoftware
    );
}

#[test]
fn source_127_task_can_take_only_pending_work_and_restore_only_into_the_empty_slot() {
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Missing),
        BluetoothModemLpTimerTaskTakeAdmission::Missing
    );
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Ready),
        BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending
    );
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::SoftwarePending),
        BluetoothModemLpTimerTaskTakeAdmission::Acquire
    );
    assert!(ready_owner_restore_is_admitted(false));
    assert!(!ready_owner_restore_is_admitted(true));
}

#[test]
fn shared_interrupt_service_retains_and_reuses_the_stable_owner() {
    let mut slot = Some(0_u8);

    assert_eq!(
        service_stable_owner(&mut slot, |owner| {
            *owner += 1;
            *owner
        }),
        Some(1)
    );
    assert_eq!(
        service_stable_owner(&mut slot, |owner| {
            *owner += 1;
            *owner
        }),
        Some(2)
    );
    assert_eq!(slot, Some(2));

    let mut missing: Option<u8> = None;
    assert_eq!(
        service_stable_owner(&mut missing, |_| panic!(
            "missing owner must not be serviced"
        )),
        None
    );
}

#[test]
fn timer_retirement_preserves_pending_owner_and_rejects_bound_routes() {
    use super::{
        EspHalBluetoothModemLpTimerRetirementError as Error, StoredModemTimerOwner as Owner,
        retire_ready_timer,
    };
    let mut ready = Some(Owner::<_, u32>::Ready(std::boxed::Box::new(109)));
    assert!(matches!(
        retire_ready_timer(&mut ready, true),
        Err(Error::RoutesBound)
    ));
    assert!(ready.is_some());
    let returned = retire_ready_timer(&mut ready, false).unwrap();
    assert_eq!(*returned, 109);
    assert!(ready.is_none());
    assert!(matches!(
        retire_ready_timer(&mut ready, false),
        Err(Error::Missing)
    ));
    let mut pending = Some(Owner::<u32, _>::SoftwarePending(std::boxed::Box::new(113)));
    assert!(matches!(
        retire_ready_timer(&mut pending, false),
        Err(Error::SoftwarePending)
    ));
    assert!(matches!(pending, Some(Owner::SoftwarePending(owner)) if *owner == 113));
}

#[test]
fn incomplete_storage_cannot_rebind_until_the_timer_is_restored() {
    use super::validate_route_owners;
    for (interrupts, timer) in [(false, false), (true, false), (false, true)] {
        assert_eq!(
            validate_route_owners(interrupts, timer),
            Err(EspHalBluetoothInterruptRouteError::OwnersUnavailable)
        );
    }
    assert_eq!(validate_route_owners(true, true), Ok(()));
}
