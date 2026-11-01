//! The shared group-key progress suite against the S31 backend: an accepted
//! station GTK mutation is drained by disable, whether its hardware change
//! is still held behind the receive cut or already applied.
use super::*;
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_testing_conformance::group_keys::{
    Completion, Mutation, Owners, ProgressCase, ProgressTrace, Timing, verify_progress,
};
use oer_ieee80211_wire::{ccmp::GroupKeyId, security::Spp};

type GroupTrace = ProgressTrace<Vec<Completion>, Vec<Owners>>;

fn key_id(id: u8) -> GroupKeyId {
    GroupKeyId::new(id).expect("KeyID")
}

/// The completion an actual group terminal names, from its returned owners.
fn completion<'id, Id: radio::SlotIdentity + 'id>(
    terminal: LowerMacTerminal<'id, Family<Id>>,
) -> Option<Completion> {
    type Keys<Id> = PortKeys<Id, Station>;
    let (mutation, slot, permission) = match terminal {
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }) => (
            Mutation::Install,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::GroupKeyReplaced { key, permission } => (
            Mutation::Replace,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::GroupKeySelected { key, permission } => (
            Mutation::Select,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { slot, permission })) => {
            (Mutation::Remove, slot.index(), permission.index())
        }
        _ => return None,
    };
    Some(Completion::Group {
        mutation,
        owners: Owners { slot, permission },
    })
}

struct DisableBody {
    mutation: Mutation,
    applied: bool,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    seen: Rc<RefCell<Option<GroupTrace>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for DisableBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let (admitted, admitted_at) = {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(Spp::NotNegotiated);
            let permission = resources.station_group_permission;
            let (key, permission, vacant) = if self.mutation == Mutation::Install {
                (None, permission, Some(slot))
            } else {
                phase.station().install_group_key(
                    &association,
                    permission,
                    slot,
                    key_id(1),
                    Ccmp128Key::new(&[1; 16]),
                );
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::Station { key, permission },
                )))) = phase.next_event().await
                else {
                    panic!("the setup installation")
                };
                (Some(key), permission, None)
            };
            let admitted = Owners {
                slot: match (&key, &vacant) {
                    (Some(key), _) => {
                        <PortKeys<Id, Station> as KeyTypes<'id>>::group_slot_identity(key)
                    }
                    (None, Some(vacant)) => vacant.index(),
                    (None, None) => panic!("a target slot"),
                },
                permission: permission.index(),
            };
            let admitted_at = self.timer.now();
            let cuts = self.observed.borrow().resumed_cuts;
            // The cut stays open until disable unless the change is applied.
            self.observed.borrow_mut().hold_cut = !self.applied;
            match self.mutation {
                Mutation::Install => phase.station().install_group_key(
                    &association,
                    permission,
                    vacant.expect("install slot"),
                    key_id(2),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Replace => phase.station().replace_group_key(
                    permission,
                    key.expect("replaced key"),
                    key_id(2),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Select => phase
                    .station()
                    .select_group_key(permission, key.expect("selected key")),
                Mutation::Remove => phase
                    .station()
                    .remove_group_key(permission, key.expect("removed key")),
            }
            for _ in 0..4 {
                barriers::yield_once().await;
            }
            assert_eq!(
                self.observed.borrow().resumed_cuts,
                cuts + usize::from(self.applied),
                "the hardware change is applied exactly when the case says"
            );
            (admitted, admitted_at)
        };
        let events = RefCell::new(Vec::new());
        let completed_at = Cell::new(admitted_at);
        let invalid = Cell::new(false);
        let mut sink = |terminal| {
            match completion::<Id>(terminal) {
                Some(event) => events.borrow_mut().push(event),
                None => invalid.set(true),
            }
            completed_at.set(self.timer.now());
            core::ops::ControlFlow::Continue(())
        };
        let mut disable = pin!(owner.disable(None, &mut sink));
        // Disable has begun while the change is still held, then the cut opens.
        let early = core::future::poll_fn(|cx| Poll::Ready(disable.as_mut().poll(cx))).await;
        assert!(early.is_pending() || self.applied);
        self.observed.borrow_mut().hold_cut = false;
        let disabled = match early {
            Poll::Ready(disabled) => disabled,
            Poll::Pending => disable.await,
        };
        assert!(!invalid.get(), "only group terminals drain");
        let poisoned = disabled.is_err();
        *self.seen.borrow_mut() = Some(ProgressTrace {
            rollback: None,
            admitted,
            timing: Timing {
                admitted_at,
                completed_at: completed_at.get(),
            },
            events: events.take(),
            retained: Vec::new(),
            poisoned,
            deadline_woke: false,
            published: None,
            blocked_while_published: false,
        });
        match disabled {
            Ok(owner) => radio::Ended::uninstall(owner, (), ()),
            Err(failed) => radio::Ended::fail_stop(failed.fail_stop(), ()),
        }
    }
}

fn exercise(mutation: Mutation, applied: bool) -> GroupTrace {
    let (parts, observed, timer) = installed(Vec::new());
    let seen = Rc::new(RefCell::new(None));
    let body = DisableBody {
        mutation,
        applied,
        timer,
        observed,
        seen: seen.clone(),
    };
    let _ = block_on(run_body(parts, body));
    seen.borrow_mut().take().expect("the drain was observed")
}

#[test]
fn the_s31_backend_drains_every_accepted_gtk_mutation_through_disable() {
    for mutation in Mutation::ALL {
        for applied in [false, true] {
            let case = ProgressCase::Disable { mutation, applied };
            assert_eq!(
                verify_progress(
                    case,
                    exercise(mutation, applied),
                    crate::port::COMMAND_BOUND
                )
                .map_err(|why| (case, why)),
                Ok(())
            );
        }
    }
}

/// An AP group MPDU published under the key holds the key's removal until
/// its own terminal returns.
struct PublishedBody {
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    seen: Rc<RefCell<Option<GroupTrace>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for PublishedBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let mut resources = resources;
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.access_point_association.into_parts();
            let (_, [slot, ..]) = bank.into_parts();
            let ap = start.begin(Spp::NotNegotiated);
            phase.access_point().install_group_key(
                &ap,
                resources.access_point_group_permission,
                slot,
                key_id(1),
                Ccmp128Key::new(&[8; 16]),
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::AccessPoint { key, permission },
            )))) = phase.next_event().await
            else {
                panic!("the AP GTK installation")
            };
            let admitted = Owners {
                slot: <PortKeys<Id, AccessPoint> as KeyTypes<'id>>::group_slot_identity(&key),
                permission: permission.index(),
            };
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            let published = oer_ieee80211_testing_conformance::group_keys::TxOwners {
                ticket: ticket.index(),
                buffer: usize::from(buffer.index()),
            };
            self.observed.borrow_mut().hold_completion = true;
            barriers::withdraw::submit_ap(&mut phase, buffer, ticket, Some(&key));
            for _ in 0..4 {
                barriers::yield_once().await;
            }
            assert_eq!(
                self.observed.borrow().publications,
                1,
                "published under the key"
            );
            let admitted_at = self.timer.now();
            phase.access_point().remove_group_key(permission, key);
            for _ in 0..4 {
                barriers::yield_once().await;
            }
            let blocked_while_published = {
                let mut event = pin!(phase.next_event());
                core::future::poll_fn(|cx| Poll::Ready(event.as_mut().poll(cx)))
                    .await
                    .is_pending()
            };
            self.observed.borrow_mut().hold_completion = false;
            let mut events = Vec::new();
            while events.len() < 2 {
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                        TxCompleted::Single(completed),
                    )))) => {
                        let returned = completed.into_parts(&mut resources.terminal_access);
                        let (buffer, _) = returned.payload.into_parts();
                        let index = match &buffer {
                            AnyBuffer::Fresh(fresh) => fresh.index(),
                            AnyBuffer::Sent(sent) => sent.index(),
                        };
                        events.push(Completion::Transmit(
                            oer_ieee80211_testing_conformance::group_keys::TxOwners {
                                ticket: returned.ticket.index(),
                                buffer: usize::from(index),
                            },
                        ));
                    }
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                        Removed::AccessPoint(RemovedKey::Group { slot, permission }),
                    )))) => events.push(Completion::Group {
                        mutation: Mutation::Remove,
                        owners: Owners {
                            slot: slot.index(),
                            permission: permission.index(),
                        },
                    }),
                    _ => panic!("an unexpected removal event"),
                }
            }
            *self.seen.borrow_mut() = Some(ProgressTrace {
                rollback: None,
                admitted,
                timing: Timing {
                    admitted_at,
                    completed_at: self.timer.now(),
                },
                events,
                retained: Vec::new(),
                poisoned: false,
                deadline_woke: false,
                published: Some(published),
                blocked_while_published,
            });
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn the_s31_backend_holds_a_gtk_removal_behind_its_published_use() {
    let (parts, observed, timer) = installed(Vec::new());
    let seen = Rc::new(RefCell::new(None));
    let body = PublishedBody {
        timer,
        observed,
        seen: seen.clone(),
    };
    assert!(matches!(
        block_on(run_body(parts, body)),
        Report::Uninstalled { .. }
    ));
    let trace = seen.borrow_mut().take().expect("the removal was observed");
    assert_eq!(
        verify_progress(
            ProgressCase::PublishedRemoval,
            trace,
            crate::port::COMMAND_BOUND
        ),
        Ok(())
    );
}

/// A GTK mutation held behind the receive cut through its bound poisons the
/// installation on the driver's own deadline wake and stays quarantined.
/// The fail-stop cancels the body, so it records each observation as it
/// makes it.
struct ExpireBody {
    mutation: Mutation,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    /// Admitted owners and instant, and whether the next due deadline was the
    /// command's own.
    admitted: Rc<Cell<Option<(Owners, Instant, bool)>>>,
    events: Rc<RefCell<Vec<Completion>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for ExpireBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(Spp::NotNegotiated);
            let permission = resources.station_group_permission;
            let (key, permission, vacant) = if self.mutation == Mutation::Install {
                (None, permission, Some(slot))
            } else {
                phase.station().install_group_key(
                    &association,
                    permission,
                    slot,
                    key_id(1),
                    Ccmp128Key::new(&[1; 16]),
                );
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::Station { key, permission },
                )))) = phase.next_event().await
                else {
                    panic!("the setup installation")
                };
                (Some(key), permission, None)
            };
            let admitted = Owners {
                slot: match (&key, &vacant) {
                    (Some(key), _) => {
                        <PortKeys<Id, Station> as KeyTypes<'id>>::group_slot_identity(key)
                    }
                    (None, Some(vacant)) => vacant.index(),
                    (None, None) => panic!("a target slot"),
                },
                permission: permission.index(),
            };
            self.observed.borrow_mut().hold_cut = true;
            let admitted_at = self.timer.now();
            match self.mutation {
                Mutation::Install => phase.station().install_group_key(
                    &association,
                    permission,
                    vacant.expect("install slot"),
                    key_id(2),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Replace => phase.station().replace_group_key(
                    permission,
                    key.expect("replaced key"),
                    key_id(2),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Select => phase
                    .station()
                    .select_group_key(permission, key.expect("selected key")),
                Mutation::Remove => phase
                    .station()
                    .remove_group_key(permission, key.expect("removed key")),
            }
            for _ in 0..4 {
                barriers::yield_once().await;
            }
            // Only the driver's own deadline is due: the harness advances to
            // it and to nothing else.
            let deadline_woke =
                self.timer.0.next_deadline() == admitted_at.checked_add(crate::port::COMMAND_BOUND);
            self.admitted
                .set(Some((admitted, admitted_at, deadline_woke)));
            let _ = self.timer.0.advance_to_next_deadline();
            loop {
                match phase.next_event().await {
                    Err(_) => break,
                    Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                        if let Some(event) = completion::<Id>(terminal) {
                            self.events.borrow_mut().push(event);
                        }
                    }
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                    Ok(_) => {}
                }
            }
        }
        match owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => radio::Ended::uninstall(owner, (), ()),
            Err(failed) => radio::Ended::fail_stop(failed.fail_stop(), ()),
        }
    }
}

#[test]
fn an_expired_gtk_mutation_poisons_on_its_own_deadline_and_stays_quarantined() {
    for mutation in Mutation::ALL {
        let (parts, observed, timer) = installed(Vec::new());
        let admitted = Rc::new(Cell::new(None));
        let events = Rc::new(RefCell::new(Vec::new()));
        let body = ExpireBody {
            mutation,
            timer: timer.clone(),
            observed,
            admitted: admitted.clone(),
            events: events.clone(),
        };
        let (poisoned, retained) = block_on(radio::with_local_slot(async |claim| {
            let mut slot = claim.place();
            let backend = Backend::new(parts).ok().expect("valid construction");
            let storage = backend.storage_for_test();
            let registered = slot.register(backend).ok().expect("fresh local slot");
            let end = radio::LocalExecution
                .scope(crate::port::Registration::guarded(registered), |frontend| {
                    radio::run_port(frontend, body)
                })
                .await;
            let poisoned = !matches!(end, radio::PortEnd::Uninstalled { .. });
            (poisoned, storage.retained_group())
        }));
        let (admitted, admitted_at, deadline_woke) =
            admitted.get().expect("the command was admitted");
        let trace = ProgressTrace {
            rollback: None,
            admitted,
            timing: Timing {
                admitted_at,
                completed_at: timer.now(),
            },
            events: events.take(),
            retained: retained
                .map(|(slot, permission)| Owners { slot, permission })
                .into_iter()
                .collect::<Vec<_>>(),
            poisoned,
            deadline_woke,
            published: None,
            blocked_while_published: false,
        };
        let case = ProgressCase::Expire(mutation);
        assert_eq!(
            verify_progress(case, trace, crate::port::COMMAND_BOUND).map_err(|why| (case, why)),
            Ok(())
        );
    }
}

/// A rejected GTK replacement write restores the old entry, quarantines the
/// replacement's owners, returns no success terminal and fails the port.
struct RollbackBody {
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    admitted: Rc<Cell<Option<(Owners, Instant)>>>,
    events: Rc<RefCell<Vec<Completion>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for RollbackBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(Spp::NotNegotiated);
            phase.station().install_group_key(
                &association,
                resources.station_group_permission,
                slot,
                key_id(1),
                Ccmp128Key::new(&[1; 16]),
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::Station { key, permission },
            )))) = phase.next_event().await
            else {
                panic!("the setup installation")
            };
            let admitted = Owners {
                slot: <PortKeys<Id, Station> as KeyTypes<'id>>::group_slot_identity(&key),
                permission: permission.index(),
            };
            self.admitted.set(Some((admitted, self.timer.now())));
            // The new write is rejected; the old entry's restoration is not.
            self.observed.borrow_mut().reject_key_writes = 1;
            phase.station().replace_group_key(
                permission,
                key,
                key_id(2),
                Ccmp128Key::new(&[2; 16]),
            );
            loop {
                match phase.next_event().await {
                    Err(_) => break,
                    Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                        if let Some(event) = completion::<Id>(terminal) {
                            self.events.borrow_mut().push(event);
                        }
                    }
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                    Ok(_) => {}
                }
            }
        }
        match owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => radio::Ended::uninstall(owner, (), ()),
            Err(failed) => radio::Ended::fail_stop(failed.fail_stop(), ()),
        }
    }
}

#[test]
fn a_rejected_gtk_replacement_restores_the_old_entry_and_quarantines_its_owners() {
    let (parts, observed, timer) = installed(Vec::new());
    let admitted = Rc::new(Cell::new(None));
    let events = Rc::new(RefCell::new(Vec::new()));
    let body = RollbackBody {
        timer: timer.clone(),
        observed: observed.clone(),
        admitted: admitted.clone(),
        events: events.clone(),
    };
    let (poisoned, retained, rolled_back) = block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        let storage = backend.storage_for_test();
        let registered = slot.register(backend).ok().expect("fresh local slot");
        let end = radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, body)
            })
            .await;
        (
            !matches!(end, radio::PortEnd::Uninstalled { .. }),
            storage.retained_group(),
            storage.rolled_back(),
        )
    }));
    // The model's own key table, read before teardown erased it.
    assert_eq!(
        observed
            .borrow()
            .material_at_abandon
            .iter()
            .map(|(_, material)| *material)
            .collect::<Vec<_>>(),
        [[1; 16]],
        "only the old material is back in the table"
    );
    let (admitted, admitted_at) = admitted.get().expect("the replacement was admitted");
    let trace = ProgressTrace {
        rollback: rolled_back.map(|(previous_key_id, attempted_key_id, restored)| {
            oer_ieee80211_testing_conformance::group_keys::Rollback {
                previous_key_id,
                attempted_key_id,
                restored,
            }
        }),
        admitted,
        timing: Timing {
            admitted_at,
            completed_at: timer.now(),
        },
        events: events.take(),
        retained: retained
            .map(|(slot, permission)| Owners { slot, permission })
            .into_iter()
            .collect::<Vec<_>>(),
        poisoned,
        deadline_woke: false,
        published: None,
        blocked_while_published: false,
    };
    assert_eq!(
        verify_progress(
            ProgressCase::ReplacementRollback,
            trace,
            crate::port::COMMAND_BOUND
        ),
        Ok(())
    );
}

/// A station data frame labelled with the material byte it travels under.
fn labelled(material: u8, released: &super::super::Released) -> TestReceive {
    let mut bytes = super::super::super::data_frame(STATION);
    bytes[25] = material;
    TestReceive {
        bytes,
        view: 0..26,
        released: released.clone(),
    }
}

/// The receive cut of one GTK mutation: old-key frames captured in each
/// stage, software queue, interrupt, DMA and hardware, all precede the
/// terminal; frames captured after it decrypt only under the new material.
/// The S31 station keeps one physical GTK slot, so InstallBeside and
/// SelectRetained, which need a second installation, do not apply.
struct CutBody {
    case: oer_ieee80211_testing_conformance::group_keys::Case,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    released: super::super::Released,
    seen: Rc<RefCell<Option<oer_ieee80211_testing_conformance::group_keys::Trace<Vec<Event>>>>>,
}
use oer_ieee80211_testing_conformance::group_keys::{Event, Installation};

/// The protocol-owned replay frontier the harness keeps per installation.
const FRONTIER: u64 = 7;

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for CutBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        use oer_ieee80211_testing_conformance::group_keys::Case;
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(9, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_)))) =
                phase.next_event().await
            else {
                panic!("configuration terminal")
            };
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(Spp::NotNegotiated);
            phase.station().install_group_key(
                &association,
                resources.station_group_permission,
                slot,
                key_id(1),
                Ccmp128Key::new(&[1; 16]),
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::Station { key, permission },
            )))) = phase.next_event().await
            else {
                panic!("the setup installation")
            };
            let original = Installation {
                identity: key.serial() as usize,
                slot: <PortKeys<Id, Station> as KeyTypes<'id>>::group_slot_identity(&key),
                counter: FRONTIER,
            };
            // Software stage: already in the port's event queue.
            self.observed
                .borrow_mut()
                .received
                .push_back(labelled(1, &self.released));
            for _ in 0..4 {
                barriers::yield_once().await;
            }
            assert!(
                self.observed.borrow().received.is_empty(),
                "delivered to software"
            );
            {
                let mut observed = self.observed.borrow_mut();
                // Interrupt stage: captured, its interrupt not yet served.
                observed.received.push_back(labelled(1, &self.released));
                // DMA and hardware stages: captured, delivered by the cut.
                observed.captured.push_back(labelled(1, &self.released));
                observed.captured.push_back(labelled(1, &self.released));
            }
            let writes = self.observed.borrow().key_writes;
            let admitted_at = self.timer.now();
            let (current, inactive) = match self.case {
                Case::Replace => {
                    phase.station().replace_group_key(
                        permission,
                        key,
                        key_id(2),
                        Ccmp128Key::new(&[2; 16]),
                    );
                    (2, 1)
                }
                Case::SelectCurrent => {
                    phase.station().select_group_key(permission, key);
                    // No installation is inactive; an unknown material stands in.
                    (1, 3)
                }
                _ => panic!("the S31 station has one GTK slot"),
            };
            let mut events = Vec::new();
            let mut completed_at = admitted_at;
            let mut continuous = false;
            let mut inactive_accepted = false;
            let mut terminal_seen = false;
            loop {
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                        let material = frame.bytes()[25];
                        phase.recycle(frame);
                        if !terminal_seen {
                            events.push(if material == 1 {
                                Event::OldRx
                            } else {
                                Event::NewRx
                            });
                        } else if material == current {
                            events.push(Event::NewRx);
                            break;
                        } else {
                            inactive_accepted = true;
                            events.push(Event::OldRx);
                        }
                    }
                    Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                        let (key, _permission) = match terminal {
                            LowerMacTerminal::GroupKeyReplaced { key, permission }
                            | LowerMacTerminal::GroupKeySelected { key, permission } => {
                                (key, permission)
                            }
                            _ => panic!("the mutation's terminal"),
                        };
                        let identity = key.serial() as usize;
                        events.push(Event::Terminal(Installation {
                            identity,
                            slot: <PortKeys<Id, Station> as KeyTypes<'id>>::group_slot_identity(
                                &key,
                            ),
                            // The harness keeps a retained installation's frontier.
                            counter: if identity == original.identity {
                                FRONTIER
                            } else {
                                0
                            },
                        }));
                        completed_at = self.timer.now();
                        continuous = self.observed.borrow().stops == 0;
                        terminal_seen = true;
                        let mut observed = self.observed.borrow_mut();
                        observed
                            .air
                            .push_back((labelled(inactive, &self.released), inactive));
                        observed
                            .air
                            .push_back((labelled(current, &self.released), current));
                    }
                    _ => panic!("an unexpected cut event"),
                }
            }
            *self.seen.borrow_mut() = Some(oer_ieee80211_testing_conformance::group_keys::Trace {
                original,
                submitted_slot: original.slot,
                timing: Timing {
                    admitted_at,
                    completed_at,
                },
                events,
                writes: self.observed.borrow().key_writes - writes,
                inactive_material_accepted: inactive_accepted,
                continuous_rx_at_terminal: continuous,
            });
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn the_s31_receive_cut_brackets_every_captured_stage_around_the_gtk_terminal() {
    use oer_ieee80211_testing_conformance::group_keys::{Case, verify};
    for case in [Case::Replace, Case::SelectCurrent] {
        let (parts, observed, timer) = installed(Vec::new());
        let seen = Rc::new(RefCell::new(None));
        let body = CutBody {
            case,
            timer,
            observed: observed.clone(),
            released: Default::default(),
            seen: seen.clone(),
        };
        assert!(matches!(
            block_on(run_body(parts, body)),
            Report::Uninstalled { .. }
        ));
        assert_eq!(
            observed.borrow().undecryptable,
            1,
            "the inactive frame was dropped"
        );
        let trace = seen.borrow_mut().take().expect("the cut was observed");
        assert_eq!(
            verify(case, trace, crate::port::COMMAND_BOUND).map_err(|why| (case, why)),
            Ok(())
        );
    }
}
