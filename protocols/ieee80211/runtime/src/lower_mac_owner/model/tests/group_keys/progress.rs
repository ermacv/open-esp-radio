use super::*;
use contract::{Completion, Mutation, Owners, ProgressCase, ProgressTrace, Timing, TxOwners};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct Wakes {
    count: AtomicUsize,
    parent: atomic_waker::AtomicWaker,
}
impl std::task::Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.parent.wake();
    }
}

pub(super) fn run_clocked<F: Future>(future: F, clock: &VirtualClock) -> F::Output {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut future = core::pin::pin!(future);
    for _ in 0..100_000 {
        let before = wakes.count.load(Ordering::SeqCst);
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        if wakes.count.load(Ordering::SeqCst) == before {
            clock.advance(oer_time::Duration::from_micros(10)).unwrap();
        }
    }
    panic!("harness did not honor its independent timer");
}

#[derive(Default)]
pub(super) struct Probe {
    admitted: Cell<Option<oer_time::Instant>>,
    deadline_woke: Cell<bool>,
}

pub(super) async fn run_timed<F: Future>(
    future: F,
    air: &ModelAir,
    clock: &VirtualClock,
    probe: Option<&Probe>,
) -> F::Output {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut future = core::pin::pin!(future);
    poll_fn(|cx| {
        wakes.parent.register(cx.waker());
        let before = wakes.count.load(Ordering::SeqCst);
        let old = oer_time::Instant::from_micros(air.now_micros.get());
        let now = clock.now();
        if now != old {
            air.set_time(now);
            if let Some(probe) = probe
                && let Some(deadline) = probe
                    .admitted
                    .get()
                    .and_then(|at| at.checked_add(MODEL_GROUP_COMMAND_BOUND))
                && old < deadline
                && now >= deadline
            {
                probe
                    .deadline_woke
                    .set(wakes.count.load(Ordering::SeqCst) > before);
            }
        }
        future
            .as_mut()
            .poll(&mut core::task::Context::from_waker(&waker))
    })
    .await
}

fn admitted(slot: u8, permission: u8) -> Owners {
    Owners { slot, permission }
}
fn terminal<'id>(
    event: LowerMacEvent<'id, ModelTypes>,
    access: &mut oer_ieee80211_lower_mac::TerminalAccess<'id, ModelTickets, ModelPool<1>>,
) -> Completion {
    let (mutation, owners) = match event {
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station {
            key,
            permission,
        })) => (
            Mutation::Install,
            admitted(key.slot.index(), permission.index()),
        ),
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
            InstalledGroup::AccessPoint { key, permission },
        )) => (
            Mutation::Install,
            admitted(key.slot.index(), permission.index()),
        ),
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyReplaced { key, permission }) => (
            Mutation::Replace,
            admitted(key.slot.index(), permission.index()),
        ),
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected { key, permission }) => (
            Mutation::Select,
            admitted(key.slot.index(), permission.index()),
        ),
        LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::Station(
            RemovedKey::Group { slot, permission },
        ))) => (Mutation::Remove, admitted(slot.index(), permission.index())),
        LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(
            RemovedKey::Group { slot, permission },
        ))) => (Mutation::Remove, admitted(slot.index(), permission.index())),
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(tx))) => {
            let tx = tx.into_parts(access);
            let (buffer, _) = tx.payload.into_parts();
            return Completion::Transmit(TxOwners {
                ticket: tx.ticket.index(),
                buffer: buffer.memory().as_ptr().addr(),
            });
        }
        _ => panic!("unexpected progress terminal"),
    };
    Completion::Group { mutation, owners }
}
fn retained(phase: &ModelPhase<'_, '_>) -> Vec<Owners> {
    fn command(change: &GroupChange<'_, ()>) -> Owners {
        match change {
            GroupChange::Install(GroupInstallationCommand::Station(pending)) => {
                admitted(pending.slot.index(), pending.permission.index())
            }
            GroupChange::Replace {
                installed: key,
                permission,
                ..
            }
            | GroupChange::Select {
                installed: key,
                permission,
            } => admitted(key.slot.index(), permission.index()),
            GroupChange::Remove {
                removed: Removed::Station(RemovedKey::Group { slot, permission }),
                ..
            } => admitted(slot.index(), permission.index()),
            _ => panic!("expiry fixture submits station GTK commands"),
        }
    }
    let mut owners = Vec::new();
    owners.extend(
        phase
            .held
            .commands
            .items
            .borrow()
            .iter()
            .filter_map(|c| match c {
                Command::Group { change, .. } => Some(command(change)),
                _ => None,
            }),
    );
    owners.extend(phase.held.failed_group.items.borrow().iter().map(command));
    for removal in &phase.held.driver_state.borrow().removing {
        if let Removed::Station(RemovedKey::Group { slot, permission }) = &removal.removed {
            owners.push(admitted(slot.index(), permission.index()));
        }
    }
    owners
}

struct Exercise<'a> {
    air: &'a ModelAir,
    case: ProgressCase,
    probe: &'a Probe,
    trace: &'a RefCell<Option<ProgressTrace>>,
}
impl Exercise<'_> {
    fn record(
        &self,
        owners: Owners,
        events: Vec<Completion>,
        retained: Vec<Owners>,
        publication: Option<(TxOwners, bool)>,
    ) {
        self.trace.replace(Some(ProgressTrace {
            admitted: owners,
            timing: Timing {
                admitted_at: self.probe.admitted.get().unwrap(),
                completed_at: oer_time::Instant::from_micros(self.air.now_micros.get()),
            },
            events,
            retained,
            poisoned: self.air.failed(),
            deadline_woke: self.probe.deadline_woke.get(),
            published: publication.map(|(tx, _)| tx),
            blocked_while_published: publication.is_some_and(|(_, blocked)| blocked),
        }));
    }
}
impl PortBody<ModelBackend> for Exercise<'_> {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        mut resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        if self.case == ProgressCase::PublishedRemoval {
            return self.published_removal(owner, resources).await;
        }
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let mut owner = owner.enable().await.ok().unwrap();
        let mutation = match self.case {
            ProgressCase::Disable { mutation, .. } | ProgressCase::Expire(mutation) => mutation,
            _ => unreachable!(),
        };
        let owned;
        {
            let mut phase = owner.phase();
            let [slot, _] = resources.station_group;
            let (key, permission, vacant) = if mutation == Mutation::Install {
                (None, resources.station_group_permission, Some(slot))
            } else {
                phase.station().install_group_key(
                    &station_association,
                    resources.station_group_permission,
                    slot,
                    id(),
                    Ccmp128Key::new(&[1; 16]),
                );
                let (key, permission) = installed(&mut phase).await;
                (Some(key), permission, None)
            };
            owned = admitted(
                key.as_ref()
                    .map_or_else(|| vacant.as_ref().unwrap().index(), |k| k.slot.index()),
                permission.index(),
            );
            self.air.hold_group_commands(true);
            self.probe.admitted.set(Some(oer_time::Instant::from_micros(
                self.air.now_micros.get(),
            )));
            match mutation {
                Mutation::Install => phase.station().install_group_key(
                    &station_association,
                    permission,
                    vacant.unwrap(),
                    id(),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Replace => phase.station().replace_group_key(
                    permission,
                    key.unwrap(),
                    id(),
                    Ccmp128Key::new(&[2; 16]),
                ),
                Mutation::Select => phase.station().select_group_key(permission, key.unwrap()),
                Mutation::Remove => phase.station().remove_group_key(permission, key.unwrap()),
            }
            match self.case {
                ProgressCase::Expire(_) => {
                    assert!(phase.next_event().await.is_err());
                    self.record(owned, Vec::new(), retained(&phase), None);
                }
                ProgressCase::Disable { applied: true, .. } => {
                    self.air.hold_group_commands(false);
                    for _ in 0..3 {
                        yield_now().await;
                    }
                }
                ProgressCase::Disable { applied: false, .. } => {}
                _ => unreachable!(),
            }
        }
        let mut events = Vec::new();
        match owner
            .disable(&mut |event| {
                events.push(terminal(
                    LowerMacEvent::Terminal(event),
                    &mut resources.terminal_access,
                ));
                core::ops::ControlFlow::Continue(())
            })
            .await
        {
            Ok(disabled) => {
                self.record(owned, events, Vec::new(), None);
                Ended::uninstall(disabled, (), ())
            }
            Err(poisoned) => {
                assert!(events.is_empty());
                Ended::uninstall(poisoned, (), ())
            }
        }
    }
}

impl Exercise<'_> {
    async fn published_removal<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        mut resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let access_point_association = ModelKeys::<AccessPoint>::begin_association(
            resources.access_point_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );
        use oer_ieee80211_lower_mac::{AttemptKey, GroupAddress};
        let mut owner = owner.enable().await.ok().unwrap();
        {
            let mut phase = owner.phase();
            let [slot, _] = resources.access_point_group;
            phase.access_point().install_group_key(
                &access_point_association,
                resources.access_point_group_permission,
                slot,
                id(),
                Ccmp128Key::new(&[1; 16]),
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::AccessPoint { key, permission },
            )))) = phase.next_event().await
            else {
                panic!("AP install")
            };
            let owners = admitted(key.slot.index(), permission.index());
            let ticket = resources.tickets.remove(0);
            let buffer = resources.small.remove(0);
            let published = TxOwners {
                ticket: ticket.index(),
                buffer: buffer.memory.as_ptr().addr(),
            };
            let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
            let frame = ClassFrame::new(TxFrame::Group {
                frame: SizedFrame::new(
                    HeaderSpec::new(
                        FrameControl::new(0x4208).unwrap(),
                        0,
                        GroupAddress::new([0xff; 6]).unwrap(),
                        ACCESS_POINT,
                        PEER,
                        0,
                        None,
                        None,
                        None,
                    )
                    .unwrap(),
                    MpduLength::new(40).unwrap(),
                )
                .unwrap(),
                rate,
            })
            .ok()
            .unwrap();
            let source =
                <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<AccessPoint, ModelCaps>(
                    (TxSource::Fresh {
                        buffer,
                        frame,
                        body: None,
                        protection: TxProtection::Ccmp(AttemptKey::Group(&key)),
                    })
                    .check()
                    .ok()
                    .unwrap(),
                )
                .ok()
                .unwrap();
            self.air
                .hold_completion(WmmAccessCategory::BestEffort, true);
            phase
                .access_point()
                .submit(
                    (oer_ieee80211_lower_mac::Attempt {
                        context: (),
                        ticket,
                        source,
                        access_category: WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                    })
                    .admit(),
                )
                .unwrap();
            for _ in 0..3 {
                yield_now().await;
            }
            assert_eq!(self.air.sent().len(), 1);
            self.probe.admitted.set(Some(oer_time::Instant::from_micros(
                self.air.now_micros.get(),
            )));
            phase.access_point().remove_group_key(permission, key);
            for _ in 0..3 {
                yield_now().await;
            }
            let blocked = phase.held.events.is_empty()
                && phase.held.driver_state.borrow().removing.len() == 1
                && phase
                    .held
                    .driver_state
                    .borrow()
                    .queues
                    .iter()
                    .any(|q| q.air.is_some());
            // Keep the publication held: its own finite hardware bound must
            // return TX before the group-removal bound, without test release.
            let mut events = Vec::new();
            for _ in 0..2 {
                events.push(terminal(
                    phase.next_event().await.unwrap().unwrap(),
                    &mut resources.terminal_access,
                ));
            }
            self.record(owners, events, Vec::new(), Some((published, blocked)));
        }
        Ended::uninstall(
            owner
                .disable(&mut |_| panic!("all terminals already returned"))
                .await
                .ok()
                .unwrap(),
            (),
            (),
        )
    }
}

pub(super) async fn exercise(case: ProgressCase, clock: &VirtualClock) -> ProgressTrace {
    let (backend, air) = backend();
    let trace = RefCell::new(None);
    let probe = Probe::default();
    let _port_end = run_timed(
        run_port(
            backend,
            Exercise {
                air: &air,
                case,
                probe: &probe,
                trace: &trace,
            },
        ),
        &air,
        clock,
        Some(&probe),
    )
    .await;
    trace.into_inner().unwrap()
}
