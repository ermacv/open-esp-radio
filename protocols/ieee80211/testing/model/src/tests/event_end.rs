use super::*;

struct Witness(Rc<Cell<usize>>);
impl Drop for Witness {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct Contexts;
impl ModelContext for Contexts {
    type Body = ModelBody;
    type OwnedContext<Id: SlotIdentity> = Witness;
    type Context<'p, Id: SlotIdentity + 'p> = Witness;
    type OwnedStationConfigureContext<Id: SlotIdentity> = Witness;
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = Witness;
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}
type Backend<Id> = ModelBackend<Id, ModelCaps, Contexts>;

#[derive(Clone, Copy)]
enum End {
    Drained,
    SubmissionFailed,
    Quarantined,
}
struct Body {
    end: End,
    posted: bool,
    drops: Rc<Cell<usize>>,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Body {
    type Output = ();
    async fn run<'a, 'id>(
        self,
        owner: Owner<'a, 'id, Id, Backend<Id>, Disabled>,
        resources: ModelResources<'id, Backend<Id>>,
    ) -> Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'a,
    {
        let mut owner = owner.enable().await.ok().unwrap();
        let ModelResources {
            mut tickets,
            mut small,
            mut terminal_access,
            station_commands,
            gate,
            ..
        } = resources;
        let air = owner.phase().held.air.clone();
        air.hold(WmmAccessCategory::BestEffort, true);
        let ticket = tickets.remove(0);
        let ticket_id = ticket.index();
        {
            let mut phase = owner.phase();
            phase.station().configure(
                station_commands
                    .configure
                    .with_context(Witness(self.drops.clone())),
                StationReceive::Off,
            );
            if matches!(self.end, End::SubmissionFailed) {
                air.fail_after_terminals();
            }
            let source = TxSource::Fresh {
                buffer: small.remove(0),
                frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                body: None,
                protection: TxProtection::Open,
            }
            .check()
            .ok()
            .unwrap();
            let source = <ModelTypes<Backend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
                Station,
                ModelCaps,
            >(source)
            .ok()
            .unwrap();
            let submitted = phase.station().submit(
                (oer_ieee80211_contracts_port::Attempt {
                    context: Witness(self.drops.clone()),
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex_operation: oer_ieee80211_contracts_port::TxOperation::Data,
                })
                .admit(),
            );
            assert_eq!(
                submitted.is_err(),
                matches!(self.end, End::SubmissionFailed)
            );
            phase.close_gate(gate);
        }
        if self.posted {
            yield_now().await;
        }
        assert_eq!(self.drops.get(), 0, "the backend still owns both contexts");
        match self.end {
            End::Drained | End::SubmissionFailed => air.fail_after_terminals(),
            End::Quarantined => air.poison(),
        }
        let mut returned = Vec::new();
        loop {
            // Every poll uses a new phase loan; the end belongs to the install.
            match owner.phase().next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(terminal))) => match terminal {
                    LowerMacTerminal::Configured(Configured::Station(permission)) => {
                        let (_permission, witness) = permission.into_parts();
                        drop(witness);
                        returned.push("configure");
                    }
                    LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
                        let parts = terminal.into_parts(&mut terminal_access);
                        assert_eq!(parts.ticket.index(), ticket_id);
                        assert!(matches!(parts.outcome, TxOutcome::Withdrawn(_)));
                        drop(parts);
                        returned.push("tx");
                    }
                    LowerMacTerminal::GateClosed(_) => returned.push("gate"),
                    _ => panic!("unexpected terminal"),
                },
                Err(end) => {
                    assert_eq!(
                        matches!(end, EventEnd::Quarantined { .. }),
                        matches!(self.end, End::Quarantined)
                    );
                    // A later failure cannot replace the final variant/cause.
                    air.poison();
                    for _ in 0..2 {
                        assert!(
                            matches!(owner.phase().next_event().await, Err(again) if again == end)
                        );
                    }
                    break;
                }
                _ => panic!("no RX or lost event in this fixture"),
            }
        }
        match self.end {
            End::Drained | End::SubmissionFailed => {
                assert_eq!(returned, ["configure", "tx", "gate"]);
                assert_eq!(self.drops.get(), 2);
                let disabled = owner
                    .disable(&mut |_| panic!("Drained returned every terminal"))
                    .await;
                let Err(poisoned) = disabled else {
                    panic!("the fault persists")
                };
                Ended::uninstall(poisoned, (), ())
            }
            End::Quarantined => {
                assert!(returned.is_empty());
                assert_eq!(self.drops.get(), 0);
                Ended::fail_stop(owner.fail_stop(), ())
            }
        }
    }
}

#[test]
fn event_end_returns_every_ordinary_terminal_or_retains_every_quarantined_owner() {
    for end in [End::Drained, End::SubmissionFailed, End::Quarantined] {
        for posted in [false, true] {
            let (backend, air) = Fixture::<ModelCaps, Contexts>::new(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
                [
                    IndividualAddress::new(STATION).unwrap(),
                    IndividualAddress::new(ACCESS_POINT).unwrap(),
                ],
                Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
            );
            let drops = Rc::new(Cell::new(0));
            let ended = block_on(fixture::inspect_model(
                backend,
                Body {
                    end,
                    posted,
                    drops: drops.clone(),
                },
                |report| match end {
                    End::Drained | End::SubmissionFailed => {
                        assert!(matches!(report, Report::Joined(_)));
                        assert_eq!(air.disable_calls(), 1);
                        assert_eq!(drops.get(), 2);
                    }
                    End::Quarantined => {
                        assert!(matches!(report, Report::Stopped(_)));
                        assert_eq!(air.disable_calls(), 0);
                        assert_eq!(
                            drops.get(),
                            0,
                            "dropping the quarantine receipt releases no owners"
                        );
                    }
                },
            ));
            drop(ended);
            assert_eq!(
                drops.get(),
                2,
                "ending the entire inert model world releases retained owners"
            );
        }
    }
}

plain_context!(Witness);

struct CancelledBody(Rc<Cell<usize>>);
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for CancelledBody {
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id>, Disabled>,
        mut resources: ModelResources<'p, Backend<Id>>,
    ) -> Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        let mut phase = owner.phase();
        phase.held.air.hold(WmmAccessCategory::BestEffort, true);
        phase.station().configure(
            resources
                .station_commands
                .configure
                .with_context(Witness(self.0.clone())),
            StationReceive::Off,
        );
        let frame = ClassFrame::new(data_frame(&phase, false))
            .ok()
            .expect("fixture frame");
        let source = prepare_model(
            TxSource::Fresh {
                buffer: resources.small.remove(0),
                frame,
                body: None,
                protection: TxProtection::Open,
            }
            .check()
            .ok()
            .expect("fixture layout"),
        )
        .ok()
        .expect("fixture preparation");
        phase
            .station()
            .submit(
                (oer_ieee80211_contracts_port::Attempt {
                    context: Witness(self.0),
                    ticket: resources.tickets.remove(0),
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).expect("fixture power"),
                    coex_operation: oer_ieee80211_contracts_port::TxOperation::Data,
                })
                .admit(),
            )
            .expect("fixture admission");
        core::future::pending().await
    }
}

#[test]
fn cancelling_the_entire_model_scope_removes_async_reachability_before_releasing_retained_owners() {
    let (world, air) = Fixture::<ModelCaps, Contexts>::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).expect("fixture channel"),
        [
            IndividualAddress::new(STATION).expect("fixture station"),
            IndividualAddress::new(ACCESS_POINT).expect("fixture AP"),
        ],
        Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
    );
    let drops = Rc::new(Cell::new(0));
    let mut running = Box::pin(run_model(world, CancelledBody(drops.clone())));
    let mut cx = core::task::Context::from_waker(Waker::noop());
    assert!(running.as_mut().poll(&mut cx).is_pending());
    assert_eq!(drops.get(), 0);
    drop(running);
    assert_eq!(air.disable_calls(), 0);
    assert_eq!(drops.get(), 2);
    assert!(air.driver.inspect().is_none());
    assert_eq!(Rc::strong_count(&air), 1);
}

#[test]
fn a_driver_wake_can_reregister_and_inject_rx_synchronously() {
    std::thread_local! { static AIR: RefCell<Option<Rc<ModelAir>>> = const { RefCell::new(None) }; }
    struct Reenter;
    impl std::task::Wake for Reenter {
        fn wake(self: std::sync::Arc<Self>) {
            AIR.with(|slot| {
                let air = slot.borrow().as_ref().unwrap().clone();
                air.driver.set(Some(Waker::noop().clone()));
                air.receive(
                    &[0x5a],
                    ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -90),
                );
            });
        }
    }
    let (_, air) = backend();
    AIR.with(|slot| *slot.borrow_mut() = Some(air.clone()));
    air.driver
        .set(Some(Waker::from(std::sync::Arc::new(Reenter))));
    air.wake_driver();
    assert_eq!(air.pending_rx(), 1);
    assert!(air.driver.take().is_none());
    AIR.with(|slot| slot.borrow_mut().take());
}
