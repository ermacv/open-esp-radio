use super::*;
use crate::lower_mac_owner::conformance::group_keys as contract;
use oer_time::{Clock, Timer};
use oer_time_virtual::VirtualClock;

mod ingress;
mod progress;

fn id() -> GroupKeyId {
    GroupKeyId::new(1).unwrap()
}
fn meta() -> ModelRxMeta {
    ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -50, -90)
}
fn snapshot(key: &ModelGroupKey<'_, Station>) -> contract::Installation {
    contract::Installation {
        identity: Rc::as_ptr(&key.identity.0).addr(),
        slot: key.slot.index(),
        counter: key.identity.0.last.get(),
    }
}
async fn installed<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
) -> (ModelGroupKey<'id, Station>, StationGroupPermission<'id>) {
    match phase.next_event().await {
        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
            InstalledGroup::Station { key, permission },
        )))) => (key, permission),
        _ => panic!("expected the real installation terminal"),
    }
}
async fn removed<'p, 'id: 'p>(phase: &mut ModelPhase<'p, 'id>) -> StationGroupPermission<'id> {
    match phase.next_event().await {
        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::Station(
            RemovedKey::Group { permission, .. },
        ))))) => permission,
        _ => panic!("expected the real removal terminal"),
    }
}

struct Boundary<'a> {
    case: contract::Case,
    air: &'a ModelAir,
    trace: &'a RefCell<Option<contract::Trace>>,
    clock: &'a VirtualClock,
}
impl Work for Boundary<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        use contract::Case;
        let [a_slot, b_slot] = resources.station_group;
        phase.station().install_group_key(
            &station_association,
            resources.station_group_permission,
            a_slot,
            id(),
            Ccmp128Key::new(&[1; 16]),
        );
        let (a, mut permission) = installed(phase).await;
        let mut spare = Some(b_slot);
        a.identity.0.last.set(73);
        let original = snapshot(&a);
        let mut retained = None;
        let (old_material, new_material) = if self.case == Case::SelectRetained {
            phase.station().install_group_key(
                &station_association,
                permission,
                spare.take().unwrap(),
                id(),
                Ccmp128Key::new(&[2; 16]),
            );
            let (b, returned) = installed(phase).await;
            permission = returned;
            b.identity.0.last.set(81);
            retained = Some(b);
            ([2; 16], [1; 16])
        } else {
            (
                [1; 16],
                if self.case == Case::SelectCurrent {
                    [1; 16]
                } else {
                    [2; 16]
                },
            )
        };
        let writes = self.air.group_writes();
        self.air.hold_group_commands(true);
        let submitted_slot = match self.case {
            Case::InstallBeside => spare.as_ref().unwrap().index(),
            _ => a.slot.index(),
        };
        match self.case {
            Case::InstallBeside => {
                phase.station().install_group_key(
                    &station_association,
                    permission,
                    spare.take().unwrap(),
                    id(),
                    Ccmp128Key::new(&[2; 16]),
                );
                retained = Some(a);
            }
            Case::Replace => {
                phase
                    .station()
                    .replace_group_key(permission, a, id(), Ccmp128Key::new(&[2; 16]))
            }
            Case::SelectRetained | Case::SelectCurrent => {
                phase.station().select_group_key(permission, a)
            }
        }
        let admitted_at = oer_time::Instant::from_micros(self.air.now_micros.get());
        self.finish(
            phase,
            (original, submitted_slot),
            (writes, admitted_at),
            old_material,
            new_material,
            retained,
        )
        .await;
    }
}

impl Boundary<'_> {
    async fn finish<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        (original, submitted_slot): (contract::Installation, u8),
        (writes, admitted_at): (usize, oer_time::Instant),
        old: [u8; 16],
        new: [u8; 16],
        retained: Option<ModelGroupKey<'id, Station>>,
    ) {
        use contract::Event;
        let mut events = Vec::new();
        assert!(self.air.receive_group(
            &[1],
            meta(),
            VifRole::Station,
            id(),
            &old,
            GroupRxStage::Ready
        ));
        let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
            panic!("RX continues while the mutation waits")
        };
        assert_eq!(frame.bytes(), &[1]);
        phase.recycle(frame);
        events.push(Event::OldRx);
        for stage in [
            GroupRxStage::Dma,
            GroupRxStage::Interrupt,
            GroupRxStage::Software,
        ] {
            assert!(
                self.air
                    .receive_group(&[1], meta(), VifRole::Station, id(), &old, stage)
            );
        }
        self.air.receive_continuously(&[0], meta());
        self.clock
            .wait_for(oer_time::Duration::from_micros(100))
            .await
            .unwrap();
        self.air.hold_group_commands(false);
        let (key, permission) = loop {
            yield_now().await;
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                    if frame.bytes() == [1] {
                        events.push(Event::OldRx);
                    } else {
                        assert_eq!(frame.bytes(), &[0]);
                    }
                    phase.recycle(frame);
                }
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::Station { key, permission },
                ))))
                | Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyReplaced {
                    key,
                    permission,
                })))
                | Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected {
                    key,
                    permission,
                }))) => {
                    break (key, permission);
                }
                _ => panic!("lost or misrouted boundary"),
            }
        };
        events.push(Event::Terminal(snapshot(&key)));
        let completed_at = oer_time::Instant::from_micros(self.air.now_micros.get());
        let continuous_rx_at_terminal = self.air.rx_stream.borrow().is_some();
        self.air.stop_continuous_receive();
        let inactive = if self.case == contract::Case::SelectCurrent {
            [3; 16]
        } else {
            old
        };
        let inactive_material_accepted = self.air.receive_group(
            &[3],
            meta(),
            VifRole::Station,
            id(),
            &inactive,
            GroupRxStage::Ready,
        );
        assert!(self.air.receive_group(
            &[2],
            meta(),
            VifRole::Station,
            id(),
            &new,
            GroupRxStage::Ready
        ));
        let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
            panic!("new installation RX follows terminal")
        };
        assert_eq!(frame.bytes(), &[2]);
        phase.recycle(frame);
        events.push(Event::NewRx);
        self.trace.replace(Some(contract::Trace {
            original,
            submitted_slot,
            timing: contract::Timing {
                admitted_at,
                completed_at,
            },
            events,
            writes: self.air.group_writes() - writes,
            inactive_material_accepted,
            continuous_rx_at_terminal,
        }));
        phase.station().remove_group_key(permission, key);
        let permission = removed(phase).await;
        if let Some(key) = retained {
            phase.station().remove_group_key(permission, key);
            let _permission = removed(phase).await;
        }
    }
}

struct Harness<'a>(&'a VirtualClock);
impl contract::Harness for Harness<'_> {
    const GROUP_COMMAND_BOUND: oer_time::Duration =
        <ModelBackend as LowerMacBackend>::GROUP_COMMAND_BOUND;
    const SCENARIO_BOUND: oer_time::Duration = oer_time::Duration::from_millis(20);
    async fn exercise(&mut self, case: contract::Case) -> contract::Trace {
        let (backend, air) = backend();
        let trace = RefCell::new(None);
        let boundary = Boundary {
            case,
            air: &air,
            trace: &trace,
            clock: self.0,
        };
        let _port_end =
            progress::run_timed(run_port(backend, Body(boundary)), &air, self.0, None).await;
        assert!(!air.failed());
        trace.into_inner().unwrap()
    }
    async fn exercise_progress(&mut self, case: contract::ProgressCase) -> contract::ProgressTrace {
        progress::exercise(case, self.0).await
    }
}
#[test]
fn common_gtk_boundary_suite_covers_delayed_ingress_and_retained_selection() {
    let clock = VirtualClock::new();
    progress::run_clocked(contract::check(&mut Harness(&clock), &clock), &clock).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Install,
    Replace,
    Select,
    Remove,
}
impl Mutation {
    const ALL: [Self; 4] = [Self::Install, Self::Replace, Self::Select, Self::Remove];
}

fn terminal_slot(event: &LowerMacEvent<'_, ModelTypes>) -> Option<u8> {
    match event {
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station {
            key,
            permission,
        }))
        | LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyReplaced { key, permission })
        | LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected { key, permission }) => {
            assert_eq!(permission.index(), 0);
            Some(key.slot.index())
        }
        LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::Station(
            RemovedKey::Group { slot, permission },
        ))) => {
            assert_eq!(permission.index(), 0);
            Some(slot.index())
        }
        _ => None,
    }
}
fn change_slot(change: &GroupChange<'_, ()>) -> u8 {
    match change {
        GroupChange::Install(GroupInstallationCommand::Station(pending)) => {
            assert_eq!(pending.permission.index(), 0);
            pending.slot.index()
        }
        GroupChange::Replace {
            installed: key,
            permission,
            ..
        }
        | GroupChange::Select {
            installed: key,
            permission,
        } => {
            assert_eq!(permission.index(), 0);
            key.slot.index()
        }
        GroupChange::Remove {
            removed: Removed::Station(RemovedKey::Group { slot, permission }),
            ..
        } => {
            assert_eq!(permission.index(), 0);
            slot.index()
        }
        _ => panic!("station fixture returned another role"),
    }
}
fn held_slots(phase: &ModelPhase<'_, '_>) -> Vec<u8> {
    let held = phase.held;
    let mut slots = Vec::new();
    for command in held.commands.items.borrow().iter() {
        if let Command::Group { change, .. } = command {
            slots.push(change_slot(change));
        }
    }
    slots.extend(held.failed_group.items.borrow().iter().map(change_slot));
    slots.extend(
        held.failed_group_terminal
            .items
            .borrow()
            .iter()
            .filter_map(terminal_slot),
    );
    slots.extend(held.events.items.borrow().iter().filter_map(terminal_slot));
    for removal in &held.driver_state.borrow().removing {
        if let Removed::Station(RemovedKey::Group { slot, permission }) = &removal.removed {
            assert_eq!(permission.index(), 0);
            slots.push(slot.index());
        }
    }
    slots
}

struct Interrupted<'a> {
    air: &'a ModelAir,
    mutation: Mutation,
    fault: Option<GroupFaultStage>,
    applied: bool,
}
impl PortBody<ModelBackend> for Interrupted<'_> {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let mut owner = owner.enable().await.ok().unwrap();
        let [slot, _] = resources.station_group;
        let expected_slot = slot.index();
        {
            let mut phase = owner.phase();
            let (permission, key, vacant) = if matches!(self.mutation, Mutation::Install) {
                (resources.station_group_permission, None, Some(slot))
            } else {
                phase.station().install_group_key(
                    &station_association,
                    resources.station_group_permission,
                    slot,
                    id(),
                    Ccmp128Key::new(&[1; 16]),
                );
                let (key, permission) = installed(&mut phase).await;
                (permission, Some(key), None)
            };
            if let Some(fault) = self.fault {
                self.air.fail_next_group_change(fault);
            }
            self.air
                .hold_group_commands(!self.applied && self.fault.is_none());
            match self.mutation {
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
            if self.fault.is_some() {
                assert!(phase.next_event().await.is_err());
            } else if self.applied {
                for _ in 0..3 {
                    yield_now().await;
                }
            }
            assert_eq!(
                held_slots(&phase),
                [expected_slot],
                "the admitted owner and permission stay together before teardown"
            );
        }
        let mut terminals = Vec::new();
        let disabled = owner
            .disable(&mut |event| {
                terminals.push(LowerMacEvent::Terminal(event));
                core::ops::ControlFlow::Continue(())
            })
            .await;
        if self.fault.is_some() {
            assert!(terminals.is_empty());
            let Err(poisoned) = disabled else {
                panic!("hardware uncertainty cannot return an ordinary owner")
            };
            Ended::fail_stopped(poisoned.fail_stop(), ())
        } else {
            let Ok(disabled) = disabled else {
                panic!("disable completes the admitted mutation")
            };
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminal_slot(&terminals[0]), Some(expected_slot));
            drop(terminals);
            Ended::uninstall(disabled, (), ())
        }
    }
}
#[test]
fn fault_and_disable_keep_each_gtk_mutations_owner_and_permission() {
    for mutation in Mutation::ALL {
        for fault in [
            Some(GroupFaultStage::Before),
            Some(GroupFaultStage::After),
            None,
        ] {
            for applied in [false, true] {
                let (backend, air) = backend();
                let _port_end = block_on(run_port(
                    backend,
                    Interrupted {
                        air: &air,
                        mutation,
                        fault,
                        applied,
                    },
                ));
                assert!(
                    air.key_installations
                        .borrow()
                        .iter()
                        .all(|entry| entry.identity.upgrade().is_none())
                );
            }
        }
    }
}

struct Deadline<'a>(&'a ModelAir);
impl Work for Deadline<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let [slot, _] = resources.station_group;
        let expected = slot.index();
        self.0.hold_group_commands(true);
        phase.station().install_group_key(
            &station_association,
            resources.station_group_permission,
            slot,
            id(),
            Ccmp128Key::new(&[1; 16]),
        );
        for _ in 0..3 {
            yield_now().await;
        }
        assert_eq!(self.0.group_writes(), 0);
        assert!(phase.next_event().await.is_err());
        assert_eq!(held_slots(phase), [expected]);
        assert_eq!(self.0.group_writes(), 0);
    }
}
#[test]
fn a_stalled_group_command_has_a_deadline_wake_and_retains_resources_on_expiry() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Wakes(AtomicUsize);
    impl std::task::Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (backend, air) = backend();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(wakes.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut running = core::pin::pin!(run_port(backend, Body(Deadline(&air))));
    for _ in 0..5 {
        assert!(running.as_mut().poll(&mut cx).is_pending());
    }
    let before = wakes.0.load(Ordering::SeqCst);
    air.set_time(oer_time::Instant::from_micros(
        MODEL_GROUP_COMMAND_BOUND.as_micros(),
    ));
    assert!(
        wakes.0.load(Ordering::SeqCst) > before,
        "expiry wakes the registered driver"
    );
    let _port_end = block_on(running);
    assert!(air.failed());
}

struct IndependentIds<'a>(&'a ModelAir);
impl Work for IndependentIds<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let [a, b] = resources.station_group;
        phase.station().install_group_key(
            &station_association,
            resources.station_group_permission,
            a,
            id(),
            Ccmp128Key::new(&[1; 16]),
        );
        let (a, permission) = installed(phase).await;
        let second = GroupKeyId::new(2).unwrap();
        phase.station().install_group_key(
            &station_association,
            permission,
            b,
            second,
            Ccmp128Key::new(&[2; 16]),
        );
        let (b, permission) = installed(phase).await;
        phase.station().select_group_key(permission, a);
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected {
            key: a,
            permission,
        }))) = phase.next_event().await
        else {
            panic!()
        };
        assert!(self.0.receive_group(
            &[1],
            meta(),
            VifRole::Station,
            id(),
            &[1; 16],
            GroupRxStage::Ready
        ));
        assert!(self.0.receive_group(
            &[2],
            meta(),
            VifRole::Station,
            second,
            &[2; 16],
            GroupRxStage::Ready
        ));
        for _ in 0..2 {
            let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
                panic!()
            };
            phase.recycle(frame);
        }
        phase.station().remove_group_key(permission, a);
        let permission = removed(phase).await;
        assert!(self.0.receive_group(
            &[2],
            meta(),
            VifRole::Station,
            second,
            &[2; 16],
            GroupRxStage::Ready
        ));
        let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
            panic!()
        };
        phase.recycle(frame);
        phase.station().remove_group_key(permission, b);
        let _permission = removed(phase).await;
    }
}
#[test]
fn selection_and_removal_leave_other_wire_key_ids_active() {
    let (backend, air) = backend();
    let _port_end = block_on(run_port(backend, Body(IndependentIds(&air))));
    assert!(!air.failed());
}

#[derive(Clone, Copy)]
enum EndInstallation {
    Uninstall,
    PoisonedUninstall,
    Abandon,
}
struct Teardown<'a> {
    air: &'a ModelAir,
    identity: &'a RefCell<Option<KeyIdentity>>,
    stage: GroupRxStage,
    end: EndInstallation,
}
impl PortBody<ModelBackend> for Teardown<'_> {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let mut owner = owner.enable().await.ok().unwrap();
        {
            let mut phase = owner.phase();
            let [slot, _] = resources.station_group;
            phase.station().install_group_key(
                &station_association,
                resources.station_group_permission,
                slot,
                id(),
                Ccmp128Key::new(&[1; 16]),
            );
            let (key, permission) = installed(&mut phase).await;
            self.identity.replace(Some(key.identity.clone()));
            assert!(self.air.receive_group(
                &[1],
                meta(),
                VifRole::Station,
                id(),
                &[1; 16],
                self.stage,
            ));
            if matches!(self.end, EndInstallation::PoisonedUninstall) {
                self.air.fail_next_group_change(GroupFaultStage::Before);
                phase.station().select_group_key(permission, key);
                assert!(phase.next_event().await.is_err());
            }
        }
        match self.end {
            EndInstallation::Uninstall => Ended::uninstall(
                owner
                    .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                    .await
                    .ok()
                    .unwrap(),
                (),
                (),
            ),
            EndInstallation::PoisonedUninstall => {
                let Err(poisoned) = owner
                    .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                    .await
                else {
                    panic!("expected poison")
                };
                Ended::uninstall(poisoned, (), ())
            }
            EndInstallation::Abandon => Ended::fail_stopped(owner.fail_stop(), ()),
        }
    }
}
#[test]
fn teardown_ends_delayed_group_ingress_and_material_despite_external_identities() {
    for end in [
        EndInstallation::Uninstall,
        EndInstallation::PoisonedUninstall,
        EndInstallation::Abandon,
    ] {
        for stage in [
            GroupRxStage::Dma,
            GroupRxStage::Interrupt,
            GroupRxStage::Software,
        ] {
            let (backend, air) = backend();
            let identity = RefCell::new(None);
            let _port_end = block_on(run_port(
                backend,
                Teardown {
                    air: &air,
                    identity: &identity,
                    stage,
                    end,
                },
            ));
            let identity = identity.into_inner().unwrap();
            assert!(!identity.0.active.get());
            assert!(!identity.0.rx_active.get());
            assert!(
                air.key_installations.borrow().is_empty(),
                "no secret survives in identity clones"
            );
            assert!(
                air.group_rx.borrow().is_empty(),
                "no captured ingress survives teardown"
            );
            assert!(!air.receive_group(
                &[2],
                meta(),
                VifRole::Station,
                id(),
                &[1; 16],
                GroupRxStage::Ready
            ));
        }
    }
}

struct ReenableGroup<'a>(&'a ModelAir);
impl PortBody<ModelBackend> for ReenableGroup<'_> {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let mut owner = owner.enable().await.ok().unwrap();
        let (key, permission) = {
            let mut phase = owner.phase();
            let [slot, _] = resources.station_group;
            phase.station().install_group_key(
                &station_association,
                resources.station_group_permission,
                slot,
                id(),
                Ccmp128Key::new(&[1; 16]),
            );
            installed(&mut phase).await
        };
        key.identity.0.last.set(41);
        let before = snapshot(&key);
        let disabled = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .unwrap();
        assert_eq!(self.0.key_installations.borrow().len(), 1);
        let mut owner = disabled.enable().await.ok().unwrap();
        assert_eq!(snapshot(&key), before);
        {
            let mut phase = owner.phase();
            assert!(self.0.receive_group(
                &[1],
                meta(),
                VifRole::Station,
                id(),
                &[1; 16],
                GroupRxStage::Ready
            ));
            let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
                panic!("reenabled installation receives")
            };
            assert_eq!(frame.bytes(), &[1]);
            phase.recycle(frame);
            phase.station().remove_group_key(permission, key);
            let _permission = removed(&mut phase).await;
            assert!(self.0.key_installations.borrow().is_empty());
        }
        Ended::uninstall(
            owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .ok()
                .unwrap(),
            (),
            (),
        )
    }
}
#[test]
fn disable_and_reenable_preserve_the_same_group_installation_and_counter() {
    let (backend, air) = backend();
    let _port_end = block_on(run_port(backend, ReenableGroup(&air)));
    assert!(!air.failed());
}

struct Admission<'a>(&'a ModelAir);
impl Work for Admission<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_mac::security::Spp::NotNegotiated,
        );

        let [slot, _] = resources.station_group;
        let slot_id = slot.index();
        self.0.hold_group_commands(true);
        phase.station().install_group_key(
            &station_association,
            resources.station_group_permission,
            slot,
            id(),
            Ccmp128Key::new(&[1; 16]),
        );
        for _ in 0..3 {
            yield_now().await;
        }
        {
            let queued = phase.held.commands.items.borrow();
            let Some(Command::Group {
                change: GroupChange::Install(GroupInstallationCommand::Station(pending)),
                ..
            }) = queued.front()
            else {
                panic!("vacant installation command remains owned")
            };
            assert_eq!(pending.slot.index(), slot_id);
            assert_eq!(pending.permission.index(), 0);
            assert_eq!(pending.key_id, id());
            assert_eq!(pending.key.as_bytes(), &[1; 16]);
        }
        assert!(phase.held.driver_state.borrow().keys.is_empty());
        assert!(phase.held.events.is_empty());
        assert!(self.0.key_installations.borrow().is_empty());
        assert_eq!(self.0.group_writes(), 0);
        assert!(!self.0.receive_group(
            &[1],
            meta(),
            VifRole::Station,
            id(),
            &[1; 16],
            GroupRxStage::Ready
        ));
        self.0.hold_group_commands(false);
        let (key, permission) = installed(phase).await;
        assert_eq!(key.slot.index(), slot_id);
        assert_eq!(self.0.group_writes(), 1);
        assert_eq!(self.0.key_installations.borrow().len(), 1);
        phase.station().remove_group_key(permission, key);
        let _permission = removed(phase).await;
    }
}
#[test]
fn pending_installation_owns_vacant_resources_without_an_installed_identity() {
    let (backend, air) = backend();
    let _port_end = block_on(run_port(backend, Body(Admission(&air))));
    assert!(!air.failed());
}
