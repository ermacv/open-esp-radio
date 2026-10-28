use super::*;

#[test]
fn unknown_interfaces_are_refused_without_selecting_another_interface() {
    let (_, handle) = OwnedModel::<LocalId<'static>>::new();
    for index in [2, u8::MAX] {
        let interface = Interface::new(index);
        let error = ModelInputError::Interface(interface);
        assert_eq!(handle.interface(interface).unwrap_err(), error);
        assert_eq!(handle.open_window(interface), Err(error));
        assert_eq!(handle.end_window(interface), Err(error));
        assert_eq!(
            handle.receive(channel(11), interface, &DATA, -20),
            Err(error)
        );
    }
    assert!(!handle.interface(Interface::PRIMARY).unwrap().configured);
    assert!(handle.0.with(|state| state.events.is_empty()));
    assert_eq!(handle.receiving(), None);
    assert!(!handle.enabled());
}

#[test]
fn invalid_frames_leave_rssi_queue_and_loss_state_unchanged_even_when_full() {
    let (_, handle) = OwnedModel::<LocalId<'static>>::new();
    let inputs = [
        (&[][..], FrameError::Empty),
        (
            &[0; 126][..],
            FrameError::TooLong {
                length: 126,
                maximum: 125,
            },
        ),
    ];
    // Input validation is independent of admission state.
    for (bytes, error) in inputs {
        assert_eq!(
            handle.receive(channel(11), Interface::PRIMARY, bytes, -10),
            Err(ModelInputError::Frame(error))
        );
    }
    handle.0.with(|state| {
        state.enabled = true;
        ModelInterface::Zero.get_mut(&mut state.interfaces).wish = Wish::Receive(channel(11));
    });
    assert!(
        handle
            .receive(channel(11), Interface::PRIMARY, &DATA, -40)
            .unwrap()
    );
    for full in [false, true] {
        if full {
            for _ in 1..MODEL_RECEIVED_FRAMES {
                assert!(
                    handle
                        .receive(channel(11), Interface::PRIMARY, &DATA, -40)
                        .unwrap()
                );
            }
        }
        let before = handle
            .0
            .with(|state| (state.events.len(), state.rssi_dbm, state.lost));
        for (bytes, error) in inputs {
            assert_eq!(
                handle.receive(channel(11), Interface::PRIMARY, bytes, -10),
                Err(ModelInputError::Frame(error))
            );
            assert_eq!(
                handle
                    .0
                    .with(|state| (state.events.len(), state.rssi_dbm, state.lost)),
                before
            );
        }
    }
    assert!(
        handle
            .receive(channel(11), Interface::PRIMARY, &DATA, -50)
            .unwrap()
    );
    assert!(handle.0.with(|state| state.lost));
    for (bytes, error) in inputs {
        assert_eq!(
            handle.receive(channel(11), Interface::PRIMARY, bytes, -10),
            Err(ModelInputError::Frame(error))
        );
    }
    assert_eq!(
        handle
            .0
            .with(|state| (state.events.len(), state.rssi_dbm, state.lost)),
        (MODEL_RECEIVED_FRAMES + 1, -50, true)
    );
}

#[test]
fn request_identity_exhaustion_preserves_the_last_id_and_returns_unadmitted_capacity() {
    struct Body;
    impl<Id: SlotIdentity> PortBody<Id, OwnedModel<Id>> for Body {
        type Output = ();
        async fn run<'a, 'p>(
            self,
            mut owner: Owner<'a, 'p, Id, OwnedModel<Id>, oer_radio_port::Disabled>,
            resources: ModelResources<'p, Id>,
        ) -> Ended<Id, OwnedModel<Id>, ()>
        where
            OwnedModel<Id>: 'p,
            Id: 'p,
            'p: 'a,
        {
            let handle = parts_handle();
            handle.0.with(|state| state.next_id = u32::MAX - 1);
            let [mut seat, _] = resources.operation;
            let mut configured = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.ok().unwrap();
            let cause = ModelFault::RequestIdExhausted { last: u32::MAX };
            {
                let mut phase = owner.phase();
                let phase = &mut *phase;
                let client = phase.clients.client(&mut configured);
                let id = client
                    .transmit(ready(client.operation(&mut seat)), transmission(&DATA))
                    .unwrap()
                    .id();
                assert_eq!(id, RequestId::new(u32::MAX));
                assert!(handle.complete_transmit(id, TxStatus::Success));
                assert!(
                    matches!(ready(phase.events.next_event()), Ok(Ok(ModelEvent::TransmitDone(_, done, _))) if done == id)
                );
                let refused = client
                    .transmit(ready(client.operation(&mut seat)), transmission(&DATA))
                    .err()
                    .unwrap();
                assert_eq!(refused.cause, cause);
                assert_eq!(handle.operation(), None);
                assert_eq!(handle.0.with(|state| state.next_id), u32::MAX);
                // The rejected admission did not retain the permit or reuse ID 0.
                let permit = ready(client.operation(&mut seat));
                assert_eq!(
                    client
                        .transmit(permit, transmission(&DATA))
                        .err()
                        .unwrap()
                        .cause,
                    cause
                );
                assert_eq!(
                    ready(phase.events.next_event()).unwrap_err(),
                    oer_radio_port::EventEnd::Drained { cause }
                );
                drop(ready(client.operation(&mut seat)));
            }
            let poisoned = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .err()
                .unwrap();
            assert_eq!(poisoned.cause(), cause);
            Ended::uninstall(poisoned, (), ())
        }
    }
    run(Body);
}

#[test]
fn a_terminal_wake_can_observe_committed_state_and_mutate_the_world() {
    std::thread_local! { static WORLD: RefCell<Option<ModelHandle>> = const { RefCell::new(None) }; }
    struct Reenter;
    impl Wake for Reenter {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            let handle = WORLD.with(|world| world.borrow().as_ref().unwrap().clone());
            assert!(handle.enabled());
            assert_eq!(handle.operation(), None);
            assert_eq!(handle.0.with(|state| state.events.len()), 1);
            handle.set_now(73);
            assert!(!handle.complete_transmit(RequestId::new(11), TxStatus::Success));
        }
    }
    let (_, handle) = OwnedModel::<LocalId<'static>>::new();
    WORLD.with(|world| *world.borrow_mut() = Some(handle.clone()));
    let waker = Rc::new(Waker::from(Arc::new(Reenter)));
    handle.0.with(|state| {
        state.enabled = true;
        state.operation = Some((Interface::PRIMARY, RequestId::new(11), Kind::Transmit));
        state.register_consumer(waker);
    });
    assert!(handle.complete_transmit(RequestId::new(11), TxStatus::Success));
    assert_eq!(handle.0.with(|state| state.now_micros), 73);
    WORLD.with(|world| *world.borrow_mut() = None);
}

#[test]
fn dropping_a_replaced_consumer_observes_the_published_replacement() {
    std::thread_local! { static WORLD: RefCell<Option<ModelHandle>> = const { RefCell::new(None) }; }
    struct OnDrop;
    impl Wake for OnDrop {
        fn wake(self: Arc<Self>) {}
    }
    impl Drop for OnDrop {
        fn drop(&mut self) {
            let handle = WORLD.with(|world| world.borrow().as_ref().unwrap().clone());
            assert!(handle.enabled());
            assert!(handle.0.with(|state| state.consumer.is_some()));
            handle.set_now(91);
        }
    }
    let (_, handle) = OwnedModel::<LocalId<'static>>::new();
    WORLD.with(|world| *world.borrow_mut() = Some(handle.clone()));
    let old = Rc::new(Waker::from(Arc::new(OnDrop)));
    handle.0.with(|state| {
        state.enabled = true;
        state.register_consumer(old);
    });
    let next = Rc::new(Waker::noop().clone());
    handle.0.with(|state| state.register_consumer(next));
    assert_eq!(handle.0.with(|state| state.now_micros), 91);
    handle.0.with(State::clear_wakers);
    WORLD.with(|world| *world.borrow_mut() = None);
}

#[test]
fn a_fault_injected_from_the_disable_sink_is_reported_by_that_disable() {
    struct Body;
    impl<Id: SlotIdentity> PortBody<Id, OwnedModel<Id>> for Body {
        type Output = ();
        async fn run<'a, 'p>(
            self,
            mut owner: Owner<'a, 'p, Id, OwnedModel<Id>, oer_radio_port::Disabled>,
            resources: ModelResources<'p, Id>,
        ) -> Ended<Id, OwnedModel<Id>, ()>
        where
            OwnedModel<Id>: 'p,
            Id: 'p,
            'p: 'a,
        {
            let handle = parts_handle();
            let [mut seat, _] = resources.operation;
            let mut configured = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.ok().unwrap();
            {
                let phase = owner.phase();
                let client = phase.clients.client(&mut configured);
                let _work = client
                    .transmit(ready(client.operation(&mut seat)), transmission(&DATA))
                    .unwrap();
            }
            let mut offered = 0;
            let poisoned = owner
                .disable(&mut |event| {
                    assert!(matches!(
                        event,
                        ModelEvent::TransmitDone(_, _, TxStatus::Aborted)
                    ));
                    offered += 1;
                    assert!(!handle.enabled());
                    handle.poison();
                    core::ops::ControlFlow::Continue(())
                })
                .await
                .err()
                .expect("the callback's fault is visible");
            assert_eq!(offered, 1);
            assert_eq!(poisoned.cause(), ModelFault::Injected);
            Ended::uninstall(poisoned, (), ())
        }
    }
    run(Body);
}
