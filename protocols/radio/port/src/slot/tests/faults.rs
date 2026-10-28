use super::*;

#[test]
fn invalid_owner_storage_is_returned_without_a_panic_or_replacement() {
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place::<Owner>();
        assert!(matches!(
            registered_owner(&mut slot.owner),
            Err(SlotStorageState::Empty)
        ));
        *slot.owner = Some(Stored::Vacant(()));
        assert!(matches!(
            registered_owner(&mut slot.owner),
            Err(SlotStorageState::Vacant)
        ));
        slot.end_model_world();
    }));
}

#[test]
fn missing_invocation_returns_its_observation_and_only_the_owner_guard_cleans_up() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, mut raw) = slot.register(Owner(model.clone())).unwrap();
        raw.invocation = None;
        let expected = LifecycleError::InvalidState(SlotObservation {
            phase: RUNNING,
            storage: SlotStorageState::Registered,
            invocation_present: false,
        });
        {
            let mut driver = pin!(raw.drive());
            assert!(matches!(poll(driver.as_mut()), Poll::Ready(Err(cause)) if cause == expected));
        }
        assert_eq!(
            model.abandons.get(),
            0,
            "portable refusal invokes no cleanup policy"
        );
        assert_eq!(model.drops.get(), 0);
        assert!(matches!(MockRegistration::new(raw).run().await, Err(cause) if cause == expected));
        assert_eq!(model.abandons.get(), 1, "the model owner selected cleanup");
        let outcome = frontend.abandon().wait().await.err().unwrap();
        assert_eq!(outcome.cause(), expected);
        assert!(matches!(outcome.outcome(), Abandon::Stopped(_)));
        slot.end_model_world();
    }));
}

#[test]
fn missing_finish_invocation_preserves_the_registered_owner() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place::<Owner>();
        *slot.owner = Some(Stored::Registered(Owner(model.clone())));
        assert_eq!(
            finish_owner(&mut slot.owner, &mut None),
            Err(SlotStorageState::Registered)
        );
        assert_eq!(model.finishes.get(), 0);
        assert_eq!(model.drops.get(), 0);
        assert!(matches!(
            storage_state(&slot.owner),
            SlotStorageState::Registered
        ));
        slot.end_model_world();
    }));
}

#[test]
fn inconsistent_reinstall_returns_the_input_and_join_without_moving_an_active_owner() {
    let model = model(true);
    for active in [false, true] {
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place::<Owner>();
            if active {
                *slot.owner = Some(Stored::Registered(Owner(model.clone())));
            }
            slot.signals.state.store(JOINED, Ordering::Relaxed);
            let invoked = Cell::new(false);
            {
                let result = slot.reinstall(
                    Joined {
                        identity: PhantomData,
                    },
                    |()| {
                        invoked.set(true);
                        Owner(model.clone())
                    },
                );
                let Err(SlotRegisterError::InvalidState {
                    cause,
                    rejected,
                    joined,
                }) = result
                else {
                    panic!("inconsistent storage must retain the reconstruction and proof");
                };
                assert_eq!(
                    cause,
                    LifecycleError::InvalidState(SlotObservation {
                        phase: JOINED,
                        storage: if active {
                            SlotStorageState::Registered
                        } else {
                            SlotStorageState::Empty
                        },
                        invocation_present: false,
                    })
                );
                assert!(!invoked.get());
                drop((rejected, joined));
            }
            assert_eq!(model.drops.get(), 0);
            slot.end_model_world();
        }));
    }
    assert_eq!(model.drops.get(), 1);
}

#[test]
fn raw_registration_drop_reports_cancellation_without_calling_hardware_policy() {
    let model = model(false);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, raw) = slot.register(Owner(model.clone())).unwrap();
        let signals = raw.signals;
        drop(raw);
        assert_eq!(signals.failure(), Some(LifecycleError::Cancelled));
        assert_eq!(model.abandons.get(), 0);
        assert_eq!(model.drops.get(), 0);
        drop(frontend);
        slot.end_model_world();
    }));
}

#[test]
fn unexpected_join_in_a_failure_wait_retains_the_observed_state() {
    let signals = Signals::new();
    signals.state.store(JOINED, Ordering::Relaxed);
    let join = SlotJoin::<LocalId<'_>> {
        signals: &signals,
        ordinary: false,
        armed: true,
        identity: PhantomData,
    };
    let outcome = finish(join.wait_failure_within(pending()));
    assert_eq!(
        outcome.cause(),
        LifecycleError::InvalidState(SlotObservation {
            phase: JOINED,
            storage: SlotStorageState::Unavailable,
            invocation_present: false,
        })
    );
}

#[test]
fn a_completed_local_runner_is_fused_instead_of_panicking_on_an_empty_output() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let mut runner = pin!(crate::LocalExecution.scope(
            guarded(slot.register(Owner(model.clone())).unwrap()),
            async |frontend| {
                drop(frontend.finish().wait().await.unwrap());
                7
            },
        ));
        assert_eq!(poll(runner.as_mut()), Poll::Ready(7));
        assert_eq!(poll(runner.as_mut()), Poll::Pending);
        assert_eq!(model.finishes.get(), 1);
    }));
}

#[test]
fn lifecycle_failure_transport_preserves_all_source_observations() {
    for phase in 0..=u8::MAX {
        for storage in [
            SlotStorageState::Empty,
            SlotStorageState::Registered,
            SlotStorageState::Vacant,
            SlotStorageState::Unavailable,
        ] {
            for invocation_present in [false, true] {
                let cause = LifecycleError::InvalidState(SlotObservation {
                    phase,
                    storage,
                    invocation_present,
                });
                assert_eq!(LifecycleError::decode(cause.encode()), Some(cause));
            }
        }
    }
    let signals = Signals::new();
    signals.request_failure(LifecycleError::FailStopRequested);
    signals.request_failure(LifecycleError::Cancelled);
    assert_eq!(signals.failure(), Some(LifecycleError::FailStopRequested));
}
