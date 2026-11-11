//! The S31 data port's group-key commands and receive path: every GTK
//! mutation's receive cut and terminal, a mutation held across a stop, its
//! expiry and a rejected replacement as named faults, the receive owners'
//! custody (recycle, lend, loss) and the ring's continuous and lent pulls;
//! beside them the EDCA table and MSDU lifetime a `SetEdca` gives the
//! station's next publications.
use super::*;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{CleanupRefused, CommandFailed};
use oer_protocols_ieee80211_testing_conformance::group_keys::{
    self as group_cases, Event, Installation, Mutation, Timing,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

type Keys<Id> = PortKeys<Id, Station>;
type StationGroupKey<'id, Id> = <Keys<Id> as KeyTypes<'id>>::GroupKey;
type StationGroupPermission<'id, Id> = <Keys<Id> as KeyTypes<'id>>::GroupPermission;
type Command<'id, Id> = PortCommand<'id, Family<Id>, Caps>;

/// The protocol-owned replay frontier the harness keeps per installation.
const FRONTIER: u64 = 7;

/// A turn's outputs in the order the port gave them.
enum Seen<'id, Id: radio::SlotIdentity + 'id> {
    Received(ReceiveBuffer<'id, Id, TestReceive>),
    Terminal(LowerMacTerminal<'id, Family<Id>>),
}

/// A sink that keeps cut units and terminals in one ordered list.
struct Ordered<'id, Id: radio::SlotIdentity + 'id>(Vec<Seen<'id, Id>>);
impl<'id, Id: radio::SlotIdentity + 'id> PortSink<'id, Family<Id>> for Ordered<'id, Id> {
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, Family<Id>>,
    ) -> Result<(), LowerMacTerminal<'id, Family<Id>>> {
        self.0.push(Seen::Terminal(terminal));
        Ok(())
    }
    fn tbtt(&mut self, _: TbttEvent) {}
    fn received(
        &mut self,
        frame: ReceiveBuffer<'id, Id, TestReceive>,
        _: crate::lower_mac::Esp32s31RxMeta,
    ) -> Result<
        (),
        (
            ReceiveBuffer<'id, Id, TestReceive>,
            crate::lower_mac::Esp32s31RxMeta,
        ),
    > {
        self.0.push(Seen::Received(frame));
        Ok(())
    }
}

/// The mutation, slot and permission a group terminal returns.
fn group_terminal<'id, Id: radio::SlotIdentity + 'id>(
    terminal: &LowerMacTerminal<'id, Family<Id>>,
) -> Option<(Mutation, u8, u8)> {
    match terminal {
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }) => Some((
            Mutation::Install,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(key),
            permission.index(),
        )),
        LowerMacTerminal::GroupKeyReplaced { key, permission } => Some((
            Mutation::Replace,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(key),
            permission.index(),
        )),
        LowerMacTerminal::GroupKeySelected { key, permission } => Some((
            Mutation::Select,
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(key),
            permission.index(),
        )),
        LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { slot, permission })) => {
            Some((Mutation::Remove, slot.index(), permission.index()))
        }
        _ => None,
    }
}

/// The fault a key-table write the hardware rejected records.
const KEY_REJECTED: Fault = Fault::Core(crate::lower_mac::LowerMacFault::Key(
    crate::mac::crypto::CryptoKeyError::HardwareRejected,
));

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// Install the station's first GTK (KeyID 1, material 1) and return
    /// the key and the permission its terminal gave back.
    fn installed_group(&mut self) -> (StationGroupKey<'id, Id>, StationGroupPermission<'id, Id>) {
        let (association, slot, _) = self.association();
        let permission = self.group_permission.take().expect("permission");
        let install =
            self.commands
                .install_group(&association, permission, slot, key_id(1), group_key(1));
        assert!(matches!(self.run(install), Ok(CommandProgress::Done)));
        match self.outbox.terminals.pop() {
            Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station {
                key,
                permission,
            })) => (key, permission),
            _ => panic!("the installation terminal returns the key"),
        }
    }

    /// The command of `mutation` on a started port, with the slot and
    /// permission it consumes: an installation into the vacant slot, or a
    /// mutation of the installed first GTK.
    fn group_command(&mut self, mutation: Mutation) -> (Command<'id, Id>, (u8, u8)) {
        if mutation == Mutation::Install {
            let (association, slot, _) = self.association();
            let permission = self.group_permission.take().expect("permission");
            let owners = (
                <Keys<Id> as KeyTypes<'id>>::vacant_group_slot_identity(&slot),
                permission.index(),
            );
            let install = self.commands.install_group(
                &association,
                permission,
                slot,
                key_id(2),
                group_key(2),
            );
            return (install, owners);
        }
        let (key, permission) = self.installed_group();
        let owners = (
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(&key),
            permission.index(),
        );
        let command = match mutation {
            Mutation::Replace => PortCommand::ReplaceGroup {
                permission,
                installed: key,
                key_id: key_id(2),
                key: group_key(2),
            },
            Mutation::Select => PortCommand::SelectGroup {
                permission,
                installed: key,
            },
            _ => PortCommand::RemoveGroup { permission, key },
        };
        (command, owners)
    }

    /// Run `command` to its end into `sink`, serving a turn between holds.
    fn run_ordered(
        &mut self,
        command: Command<'id, Id>,
        sink: &mut Ordered<'id, Id>,
    ) -> CommandProgress<'id, Family<Id>, Caps> {
        let mut command = command;
        for _ in 0..16 {
            let now = self.now();
            match self.port.command(command, now, sink).unwrap() {
                Ok(CommandProgress::Held(held)) => command = held,
                Ok(other) => return other,
                Err(fault) => panic!("the command faulted: {fault:?}"),
            }
            self.turn();
        }
        panic!("the command ends");
    }

    /// A station data frame labelled with the material byte it travels
    /// under.
    fn labelled(&self, material: u8) -> TestReceive {
        let mut frame = self.frame();
        frame.bytes[25] = material;
        frame
    }

    /// The group terminals in the outbox.
    fn group_terminals(&self) -> Vec<(Mutation, u8, u8)> {
        self.outbox
            .terminals
            .iter()
            .filter_map(group_terminal::<Id>)
            .collect()
    }
}

#[test]
fn every_group_mutation_runs_behind_one_finite_rx_cut_and_returns_its_owners_once() {
    with_fixture(|fixture| {
        fixture.start();
        let (key, permission) = fixture.installed_group();
        let slot = <Keys<_> as KeyTypes<'_>>::group_slot_identity(&key);
        let index = permission.index();
        let replace = PortCommand::ReplaceGroup {
            permission,
            installed: key,
            key_id: key_id(2),
            key: group_key(2),
        };
        assert!(matches!(fixture.run(replace), Ok(CommandProgress::Done)));
        let (key, permission) = match fixture.outbox.terminals.pop() {
            Some(LowerMacTerminal::GroupKeyReplaced { key, permission }) => (key, permission),
            _ => panic!("replaced"),
        };
        assert_eq!(
            (
                <Keys<_> as KeyTypes<'_>>::group_slot_identity(&key),
                permission.index()
            ),
            (slot, index)
        );
        let select = PortCommand::SelectGroup {
            permission,
            installed: key,
        };
        assert!(matches!(fixture.run(select), Ok(CommandProgress::Done)));
        let (key, permission) = match fixture.outbox.terminals.pop() {
            Some(LowerMacTerminal::GroupKeySelected { key, permission }) => (key, permission),
            _ => panic!("selected"),
        };
        let remove = PortCommand::RemoveGroup { permission, key };
        assert!(matches!(fixture.run(remove), Ok(CommandProgress::Done)));
        assert_eq!(
            fixture.group_terminals(),
            [(Mutation::Remove, slot, index)],
            "the removal returns the slot and the permission"
        );
        assert!(
            fixture.outbox.terminals.len() == 1,
            "each mutation ended in exactly one terminal"
        );
        // Install, replace, select and remove each closed and reopened one cut.
        assert_eq!(fixture.observed.borrow().resumed_cuts, 4);
        assert_eq!(fixture.observed.borrow().stops, 0, "no mutation stopped RX");
        fixture.stop();
    });
}

#[test]
fn the_receive_cut_brackets_every_captured_stage_around_the_gtk_terminal() {
    use group_cases::Case;
    // The S31 station keeps one GTK slot: InstallBeside and SelectRetained,
    // which need a second installation, do not apply.
    for case in [Case::Replace, Case::SelectCurrent] {
        with_fixture(|fixture| {
            fixture.start();
            fixture.configure_station();
            let (key, permission) = fixture.installed_group();
            let original = Installation {
                identity: usize::try_from(key.serial()).expect("serial"),
                slot: <Keys<_> as KeyTypes<'_>>::group_slot_identity(&key),
                counter: FRONTIER,
            };
            // Ring stage: staged for the caller's pull. The transmit turns
            // run meanwhile and leave it there.
            let staged = fixture.labelled(1);
            fixture.observed.borrow_mut().received.push_back(staged);
            for _ in 0..4 {
                fixture.turn();
            }
            assert_eq!(
                fixture.observed.borrow().received.len(),
                1,
                "staged for the pull"
            );
            {
                let interrupt = fixture.labelled(1);
                let dma = fixture.labelled(1);
                let hardware = fixture.labelled(1);
                let mut observed = fixture.observed.borrow_mut();
                // Interrupt stage: captured, its interrupt not yet served.
                observed.received.push_back(interrupt);
                // DMA and hardware stages: captured, delivered by the cut.
                observed.captured.push_back(dma);
                observed.captured.push_back(hardware);
            }
            let writes = fixture.observed.borrow().key_writes;
            let admitted_at = fixture.now();
            let (command, current, inactive) = match case {
                Case::Replace => (
                    PortCommand::ReplaceGroup {
                        permission,
                        installed: key,
                        key_id: key_id(2),
                        key: group_key(2),
                    },
                    2,
                    1,
                ),
                // No installation is inactive; an unknown material stands in.
                _ => (
                    PortCommand::SelectGroup {
                        permission,
                        installed: key,
                    },
                    1,
                    3,
                ),
            };
            let mut ordered = Ordered(Vec::new());
            assert!(matches!(
                fixture.run_ordered(command, &mut ordered),
                CommandProgress::Done
            ));
            let completed_at = fixture.now();
            let continuous = fixture.observed.borrow().stops == 0;
            let mut events = Vec::new();
            let mut terminal = None;
            for seen in ordered.0 {
                match seen {
                    Seen::Received(frame) => {
                        assert!(terminal.is_none(), "a cut unit after the terminal");
                        events.push(if frame.bytes()[25] == 1 {
                            Event::OldRx
                        } else {
                            Event::NewRx
                        });
                        assert!(fixture.port.recycle(frame).is_ok());
                    }
                    Seen::Terminal(
                        LowerMacTerminal::GroupKeyReplaced { key, permission }
                        | LowerMacTerminal::GroupKeySelected { key, permission },
                    ) => {
                        let identity = usize::try_from(key.serial()).expect("serial");
                        events.push(Event::Terminal(Installation {
                            identity,
                            slot: <Keys<_> as KeyTypes<'_>>::group_slot_identity(&key),
                            // The harness keeps a retained installation's
                            // frontier.
                            counter: if identity == original.identity {
                                FRONTIER
                            } else {
                                0
                            },
                        }));
                        terminal = Some((key, permission));
                    }
                    Seen::Terminal(_) => panic!("the mutation's terminal"),
                }
            }
            assert!(terminal.is_some(), "the mutation ended in its terminal");
            {
                let inactive_frame = fixture.labelled(inactive);
                let current_frame = fixture.labelled(current);
                let mut observed = fixture.observed.borrow_mut();
                observed.air.push_back((inactive_frame, inactive));
                observed.air.push_back((current_frame, current));
            }
            let now = fixture.now();
            let mut inactive_accepted = false;
            while let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") {
                if frame.bytes()[25] == current {
                    events.push(Event::NewRx);
                } else {
                    inactive_accepted = true;
                    events.push(Event::OldRx);
                }
                assert!(fixture.port.recycle(frame).is_ok());
            }
            assert_eq!(
                fixture.observed.borrow().undecryptable,
                1,
                "the inactive frame was dropped"
            );
            let trace = group_cases::Trace {
                original,
                submitted_slot: original.slot,
                timing: Timing {
                    admitted_at,
                    completed_at,
                },
                events,
                writes: fixture.observed.borrow().key_writes - writes,
                inactive_material_accepted: inactive_accepted,
                continuous_rx_at_terminal: continuous,
            };
            assert_eq!(
                group_cases::verify(case, trace, COMMAND_BOUND).map_err(|why| (case, why)),
                Ok(())
            );
        });
    }
}

#[test]
fn a_gtk_mutation_behind_a_published_frame_outlasts_its_bound_while_the_frame_holds_it() {
    with_fixture(|fixture| {
        fixture.start();
        let (mut command, _) = fixture.group_command(Mutation::Select);
        // The air keeps the frame on its queue, and a configuration holds
        // every station frame ahead of the mutation.
        fixture.hold_air(true);
        let attempt = fixture.station_mpdu().expect("attempt");
        let now = fixture.now();
        fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .unwrap()
            .ok()
            .expect("published");
        let configure = DataPortHarness::configure(fixture).expect("permission");
        let now = fixture.now();
        assert!(
            fixture
                .port
                .command(configure, now, &mut fixture.outbox)
                .unwrap()
                .is_ok()
        );
        for _ in 0..6 {
            let now = fixture.now();
            match fixture
                .port
                .command(command, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Held(held)) => command = held,
                other => panic!("the frame holds the mutation: {:?}", other.is_ok()),
            }
            fixture.advance(COMMAND_BOUND.saturating_sub(Duration::from_millis(1)));
        }
        assert_eq!(fixture.port.fault(), None);
    });
}

#[test]
fn a_gtk_mutation_held_at_its_cut_comes_back_refused_by_a_stopped_port_and_runs_after_restart() {
    for mutation in Mutation::ALL {
        with_fixture(|fixture| {
            fixture.start();
            let (command, owners) = fixture.group_command(mutation);
            let cuts = fixture.observed.borrow().resumed_cuts;
            fixture.observed.borrow_mut().hold_cut = true;
            let now = fixture.now();
            let held = match fixture
                .port
                .command(command, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Held(held)) => held,
                _ => panic!("{mutation:?}: the cut holds the mutation"),
            };
            fixture.stop();
            // The stopped port returns the command and its owners unchanged.
            let now = fixture.now();
            let refused = match fixture
                .port
                .command(held, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Refused(refused, CommandRefusal::NotEnabled)) => refused,
                _ => panic!("{mutation:?}: a stopped port refuses the held mutation"),
            };
            assert!(fixture.group_terminals().is_empty(), "{mutation:?}");
            assert_eq!(fixture.observed.borrow().resumed_cuts, cuts);
            // The caller runs it again, first, once the port runs again.
            fixture.observed.borrow_mut().hold_cut = false;
            fixture.start();
            assert!(
                matches!(fixture.run(refused), Ok(CommandProgress::Done)),
                "{mutation:?}"
            );
            assert_eq!(
                fixture.group_terminals(),
                [(mutation, owners.0, owners.1)],
                "{mutation:?}: one terminal returns the owners it took"
            );
            assert_eq!(fixture.observed.borrow().resumed_cuts, cuts + 1);
            fixture.stop();
        });
    }
}

#[test]
fn a_gtk_mutation_waits_for_units_the_network_holds_past_its_bound() {
    with_fixture(|fixture| {
        fixture.start();
        let (command, owners) = fixture.group_command(Mutation::Replace);
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.hold_cut = true;
            observed.cut_by_units = true;
        }
        let now = fixture.now();
        let mut held = match fixture
            .port
            .command(command, now, &mut fixture.outbox)
            .unwrap()
        {
            Ok(CommandProgress::Held(held)) => held,
            _ => panic!("the cut holds the mutation"),
        };
        // The network keeps its units for longer than a command's bound.
        for _ in 0..3 {
            fixture.advance(Duration::from_micros(COMMAND_BOUND.as_micros() - 1000));
            let now = fixture.now();
            held = match fixture
                .port
                .command(held, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Held(held)) => held,
                _ => panic!("the units decide: {:?}", fixture.port.fault()),
            };
        }
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.hold_cut = false;
            observed.cut_by_units = false;
        }
        assert!(matches!(fixture.run(held), Ok(CommandProgress::Done)));
        assert_eq!(
            fixture.group_terminals(),
            [(Mutation::Replace, owners.0, owners.1)]
        );
        fixture.stop();
    });
}

#[test]
fn a_fault_that_took_owners_keeps_the_port_quarantined() {
    with_fixture(|fixture| {
        fixture.start();
        // Each of these left a site with an owner in its hands.
        for fault in [
            Fault::RemovalMismatch,
            Fault::ReceiveAccounting,
            Fault::KeySlotMissing(crate::port::KeyOp::Remove),
            // The core recorded its own fault behind this refusal.
            Fault::Publish {
                queue: 0,
                step: crate::port::PublishStep::KeyAdmission,
            },
        ] {
            assert!(!crate::port::Fault::leaves_owners_whole(fault), "{fault:?}");
        }
    });
}

#[test]
fn a_fault_on_a_descriptor_keeps_the_port_quarantined() {
    with_fixture(|fixture| {
        fixture.start();
        let fault = Fault::Publish {
            queue: 1,
            step: PublishStep::Attach,
        };
        fixture.port.fault_for_test(fault);
        assert!(!fixture.port.recover_for_stop());
        assert_eq!(fixture.port.fault(), Some(fault));
    });
}

#[test]
fn a_gtk_mutation_refused_by_a_stop_measures_its_bound_from_the_restart() {
    with_fixture(|fixture| {
        fixture.start();
        let (command, owners) = fixture.group_command(Mutation::Replace);
        fixture.observed.borrow_mut().hold_cut = true;
        let now = fixture.now();
        let held = match fixture
            .port
            .command(command, now, &mut fixture.outbox)
            .unwrap()
        {
            Ok(CommandProgress::Held(held)) => held,
            _ => panic!("the cut holds the mutation"),
        };
        fixture.stop();
        let now = fixture.now();
        let refused = match fixture
            .port
            .command(held, now, &mut fixture.outbox)
            .unwrap()
        {
            Ok(CommandProgress::Refused(refused, CommandRefusal::NotEnabled)) => refused,
            _ => panic!("a stopped port refuses the held mutation"),
        };
        // The radio stays stopped longer than a command's bound.
        fixture.advance(COMMAND_BOUND);
        fixture.observed.borrow_mut().hold_cut = false;
        fixture.start();
        assert!(
            matches!(fixture.run(refused), Ok(CommandProgress::Done)),
            "the stopped time is not the command's: {:?}",
            fixture.port.fault()
        );
        assert_eq!(
            fixture.group_terminals(),
            [(Mutation::Replace, owners.0, owners.1)]
        );
        fixture.stop();
    });
}

#[test]
fn a_gtk_mutation_refused_by_a_stop_leaves_no_hold_on_single_mpdus_after_the_restart() {
    for mutation in [Mutation::Install, Mutation::Replace, Mutation::Remove] {
        with_fixture(|fixture| {
            fixture.start();
            let (command, _) = fixture.group_command(mutation);
            fixture.observed.borrow_mut().hold_cut = true;
            let now = fixture.now();
            let held = match fixture
                .port
                .command(command, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Held(held)) => held,
                _ => panic!("{mutation:?}: the cut holds the mutation"),
            };
            fixture.stop();
            let now = fixture.now();
            let refused = match fixture
                .port
                .command(held, now, &mut fixture.outbox)
                .unwrap()
            {
                Ok(CommandProgress::Refused(refused, CommandRefusal::NotEnabled)) => refused,
                _ => panic!("{mutation:?}: a stopped port refuses the held mutation"),
            };
            // The restarted port runs without the refused command: its
            // single MPDUs publish at once, its deadline names no barrier.
            fixture.observed.borrow_mut().hold_cut = false;
            fixture.start();
            assert_eq!(fixture.port.deadline(), None, "{mutation:?}");
            let publications = fixture.observed.borrow().publications;
            let attempt = fixture.station_mpdu().expect("attempt");
            let now = fixture.now();
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .unwrap()
                .unwrap_or_else(|_| panic!("{mutation:?}: submitted"));
            // The device reports its publications at the next turn's facts.
            let _ = fixture.turn();
            assert_eq!(
                fixture.observed.borrow().publications,
                publications + 1,
                "{mutation:?}: the restarted port publishes the single MPDU"
            );
            assert_eq!(fixture.port.fault(), None, "{mutation:?}");
            drop(refused);
            fixture.stop();
        });
    }
}

#[test]
fn every_stalled_gtk_mutation_is_refused_expired_at_its_own_deadline_and_runs_again_later() {
    // An installation's expiry is checked beside the cut-held installation
    // in data_port.rs; these are the mutations of an installed key.
    for mutation in [Mutation::Replace, Mutation::Select, Mutation::Remove] {
        with_fixture(|fixture| {
            fixture.start();
            let (command, _) = fixture.group_command(mutation);
            let cuts = fixture.observed.borrow().resumed_cuts;
            fixture.observed.borrow_mut().hold_cut = true;
            let admitted = fixture.now();
            let bound = admitted.saturating_add(COMMAND_BOUND);
            let mut command = command;
            for _ in 0..100_000 {
                let now = fixture.now();
                match fixture
                    .port
                    .command(command, now, &mut fixture.outbox)
                    .unwrap()
                {
                    Ok(CommandProgress::Held(held)) => command = held,
                    Ok(CommandProgress::Refused(refused, CommandRefusal::Expired)) => {
                        assert_eq!(now, bound, "{mutation:?}: expired at its own bound");
                        assert_eq!(fixture.port.fault(), None, "{mutation:?}");
                        assert!(fixture.group_terminals().is_empty(), "{mutation:?}");
                        // The cut it polled ended; nothing resumed it as a
                        // finished mutation.
                        assert_eq!(fixture.observed.borrow().abandoned_cuts, 1);
                        assert_eq!(fixture.observed.borrow().resumed_cuts, cuts);
                        assert_eq!(fixture.port.deadline(), None, "{mutation:?}");
                        // The port runs on, and the key is still installed:
                        // the same command runs once the cut passes.
                        fixture.observed.borrow_mut().hold_cut = false;
                        assert!(
                            matches!(fixture.run(refused), Ok(CommandProgress::Done)),
                            "{mutation:?}: the refused command runs again"
                        );
                        assert_eq!(fixture.group_terminals().len(), 1, "{mutation:?}");
                        fixture.stop();
                        assert_eq!(fixture.port.fault(), None, "{mutation:?}");
                        return;
                    }
                    Err(fault) => panic!("{mutation:?}: {fault:?}"),
                    Ok(_) => panic!("{mutation:?}: the cut holds the mutation"),
                }
                // The caller follows the port's deadline, which the held
                // command bounds.
                let at = fixture
                    .port
                    .deadline()
                    .expect("the held command's deadline");
                assert!(at > now && at <= bound, "{mutation:?}: {at:?}");
                fixture.advance(at.saturating_duration_since(now));
                fixture.turn();
            }
            panic!("{mutation:?}: the held mutation expires");
        });
    }
}

/// The replacement's KeyID and material the tests attempt.
const REPLACEMENT: (u8, u8) = (2, 2);

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// The installed first GTK's replacement, with the slot and
    /// permission it holds.
    fn replacement(&mut self) -> (Command<'id, Id>, (u8, u8)) {
        let (key, permission) = self.installed_group();
        let owners = (
            <Keys<Id> as KeyTypes<'id>>::group_slot_identity(&key),
            permission.index(),
        );
        let replace = PortCommand::ReplaceGroup {
            permission,
            installed: key,
            key_id: key_id(REPLACEMENT.0),
            key: group_key(REPLACEMENT.1),
        };
        (replace, owners)
    }

    /// Whether the stopped port refuses its release once the fixture's
    /// transmit buffers went home.
    fn stopped_release_refused(&mut self) -> bool {
        drop(core::mem::take(&mut self.buffers));
        self.port.release().is_none()
    }

    /// One service of the stopped port: its result as the port gave it.
    fn stopped_service(&mut self) -> Result<(), Fault> {
        let now = self.now();
        self.port
            .service_tx(TxFacts::DUE, false, now, &mut self.outbox)
            .unwrap()
            .map(|completed| assert!(completed.is_none()))
    }

    /// The lifecycle owner's cleanup of the stopped port.
    fn clean(&mut self) -> Result<(), CleanupRefused<Fault>> {
        self.port.clean_stopped(&mut self.outbox).unwrap()
    }
}

/// Whether the stopped port retires once the fixture's transmit buffers
/// went home, as every retire check needs.
fn retires<Id: radio::SlotIdentity>(fixture: &mut Fixture<'_, Id>) {
    drop(core::mem::take(&mut fixture.buffers));
    assert!(fixture.port.release().is_some(), "the stopped port retires");
}

/// A replacement the hardware rejects and rolls back is a named fault with
/// its command whole: the previous key, its identity and its slot and
/// permission come back, its entry is in the table again, the receive cut
/// resumes, and a stop recovers the port, which retires.
#[test]
fn a_gtk_replacement_the_hardware_rolls_back_comes_back_whole_beside_its_fault() {
    with_installations(|mut fixture, _| {
        let fixture = &mut fixture;
        fixture.start();
        let (replace, owners) = fixture.replacement();
        let cuts = fixture.observed.borrow().resumed_cuts;
        // The new write is rejected; the old entry's restoration is not.
        fixture.observed.borrow_mut().reject_key_writes = 1;
        let returned = match fixture.run_failed(replace) {
            Err(CommandFailed::Returned { command, fault }) => {
                assert_eq!(
                    fault,
                    Fault::GroupReplacement(GroupReplacement::RolledBack(
                        crate::mac::crypto::CryptoKeyError::HardwareRejected
                    ))
                );
                command
            }
            _ => panic!("the replacement comes back beside its fault"),
        };
        match returned {
            PortCommand::ReplaceGroup {
                permission,
                installed,
                key_id: attempted,
                ..
            } => {
                assert_eq!(
                    (
                        <Keys<_> as KeyTypes<'_>>::group_slot_identity(&installed),
                        permission.index()
                    ),
                    owners,
                    "the very key and permission"
                );
                assert_eq!(attempted.get(), REPLACEMENT.0);
                drop((permission, installed));
            }
            _ => panic!("the replacement itself"),
        }
        // Nothing is retained; the rollback restored the previous KeyID.
        assert_eq!(fixture.port.retained_group(), (None, Some((1, 2, true))));
        assert!(fixture.group_terminals().is_empty(), "no terminal");
        assert_eq!(fixture.observed.borrow().resumed_cuts, cuts + 1);
        // The stop takes the fault back and the stopped port retires.
        assert!(fixture.port.recover_for_stop());
        fixture.stop();
        assert!(fixture.stopped_service().is_ok());
        assert!(fixture.group_terminals().is_empty());
        retires(fixture);
    });
}

/// A replacement whose rollback fails keeps both causes and its owners in
/// the port: no terminal, and the stopped port does not retire until its
/// cleanup cleared the entry and read it cleared; then the key's removal
/// returns its slot and the command's permission, once, and the port
/// retires.
#[test]
fn a_gtk_replacement_whose_rollback_fails_returns_its_owners_through_the_stop_once() {
    with_installations(|mut fixture, _| {
        let fixture = &mut fixture;
        fixture.start();
        let (replace, owners) = fixture.replacement();
        let cuts = fixture.observed.borrow().resumed_cuts;
        // The new write and the old entry's restoration are both rejected.
        fixture.observed.borrow_mut().reject_key_writes = 2;
        let rejected = crate::mac::crypto::CryptoKeyError::HardwareRejected;
        let failed = Fault::GroupReplacement(GroupReplacement::RollbackFailed {
            replacement: rejected,
            rollback: rejected,
        });
        assert!(matches!(
            fixture.run_failed(replace),
            Err(CommandFailed::Taken(fault)) if fault == failed
        ));
        assert_eq!(fixture.port.fault(), Some(failed));
        assert_eq!(
            fixture.port.retained_group(),
            (Some(owners), Some((1, 2, false)))
        );
        assert!(fixture.group_terminals().is_empty(), "no terminal");
        assert_eq!(fixture.observed.borrow().resumed_cuts, cuts + 1);
        // Before a confirmed stop the cleanup refuses and writes nothing.
        let clears = fixture.observed.borrow().key_clears;
        assert_eq!(fixture.clean(), Err(CleanupRefused::NotStopped));
        assert_eq!(fixture.observed.borrow().key_clears, clears);
        // The stop takes the fault back; the stopped port keeps the owners
        // until its cleanup.
        assert!(fixture.port.recover_for_stop());
        fixture.stop();
        assert!(fixture.stopped_release_refused(), "the owners are retained");
        // The stopped service is not the cleanup: it neither frees the
        // owners nor reports the cleanup's result.
        let clears = fixture.observed.borrow().key_clears;
        assert!(fixture.stopped_service().is_ok());
        assert!(fixture.group_terminals().is_empty());
        assert_eq!(fixture.observed.borrow().key_clears, clears);
        assert!(fixture.stopped_release_refused(), "still retained");
        assert!(fixture.clean().is_ok());
        assert_eq!(
            fixture.group_terminals(),
            [(Mutation::Remove, owners.0, owners.1)],
            "the removal returns the slot and the permission"
        );
        assert!(fixture.clean().is_ok());
        assert!(fixture.stopped_service().is_ok());
        assert_eq!(fixture.group_terminals().len(), 1, "once");
        retires(fixture);
    });
}

/// The cleanup of a failed rollback's entry, each case its own: without
/// registers it writes nothing and waits; a table that still reads the
/// entry valid after the one clear frees nothing, and each later cleanup
/// call only reads it again. Each is the call's named result, the port's
/// own fault untouched, the owners with the port. Once the table reads the
/// entry cleared, the removal (writing nothing more) returns the same slot
/// and permission once, and the port retires.
#[test]
fn a_failed_rollbacks_entry_is_cleared_once_and_then_only_read_until_it_reads_cleared() {
    with_installations(|mut fixture, _| {
        let fixture = &mut fixture;
        fixture.start();
        let (replace, owners) = fixture.replacement();
        fixture.observed.borrow_mut().reject_key_writes = 2;
        assert!(matches!(
            fixture.run_failed(replace),
            Err(CommandFailed::Taken(_))
        ));
        assert!(fixture.port.recover_for_stop());
        fixture.stop();
        // The device's `hardware` records the clears earlier calls wrote.
        let observed = fixture.observed.clone();
        let clears = || observed.borrow().key_clears;
        fixture.observed.borrow_mut().hardware_unavailable = true;
        assert_eq!(
            fixture.clean(),
            Err(CleanupRefused::Unavailable(Fault::HardwareUnavailable))
        );
        let before = clears();
        fixture.observed.borrow_mut().hardware_unavailable = false;
        fixture.observed.borrow_mut().key_entries_stuck = true;
        assert_eq!(
            fixture.clean(),
            Err(CleanupRefused::Held(Fault::GroupEntryHeld))
        );
        assert_eq!(clears(), before, "nothing was written without registers");
        assert!(fixture.group_terminals().is_empty(), "the slot is not free");
        assert_eq!(
            fixture.port.fault(),
            None,
            "the call's result, not recorded"
        );
        assert!(fixture.port.retained_group().0.is_some());
        assert!(fixture.stopped_release_refused(), "the owners stay");
        // Read again: still valid.
        assert_eq!(
            fixture.clean(),
            Err(CleanupRefused::Held(Fault::GroupEntryHeld))
        );
        assert_eq!(clears(), before + 1, "the clear was issued once");
        assert_eq!(
            fixture.clean(),
            Err(CleanupRefused::Held(Fault::GroupEntryHeld))
        );
        assert_eq!(clears(), before + 1, "a read writes nothing");
        assert!(fixture.group_terminals().is_empty());
        // The table reads the entry cleared: the removal, which writes the
        // entry no more, returns the owners once.
        fixture.observed.borrow_mut().key_entries_stuck = false;
        assert!(fixture.clean().is_ok());
        assert_eq!(
            fixture.group_terminals(),
            [(Mutation::Remove, owners.0, owners.1)],
            "the removal returns the slot and the permission"
        );
        assert!(fixture.clean().is_ok());
        assert_eq!(clears(), before + 1, "no second clear");
        assert_eq!(fixture.group_terminals().len(), 1, "once");
        retires(fixture);
    });
}

/// The port's release does not ask for a cleanup receipt's owners: a
/// caller that dropped the receipt before it would still see the port
/// retire. So the receipt leaves whole to the caller's sink, and the caller
/// keeps the very slot and permission past the release.
#[test]
fn a_cleanup_receipt_leaves_whole_to_its_caller_and_outlives_the_release() {
    with_installations(|mut fixture, _| {
        let fixture = &mut fixture;
        fixture.start();
        let (replace, owners) = fixture.replacement();
        fixture.observed.borrow_mut().reject_key_writes = 2;
        assert!(matches!(
            fixture.run_failed(replace),
            Err(CommandFailed::Taken(_))
        ));
        assert!(fixture.port.recover_for_stop());
        fixture.stop();
        assert!(fixture.clean().is_ok());
        let index = fixture
            .outbox
            .terminals
            .iter()
            .position(|terminal| group_terminal::<_>(terminal).is_some())
            .expect("the receipt is in the caller's sink");
        let receipt = fixture.outbox.terminals.remove(index);
        assert_eq!(
            group_terminal::<_>(&receipt),
            Some((Mutation::Remove, owners.0, owners.1))
        );
        // Not enforced by the port: the release succeeds with the receipt
        // out, whether its holder keeps it or dropped it.
        retires(fixture);
        assert_eq!(
            group_terminal::<_>(&receipt),
            Some((Mutation::Remove, owners.0, owners.1)),
            "the caller still holds the owners after the release"
        );
        drop(receipt);
    });
}

/// A stopped port that holds a fault runs no cleanup: the refusal names
/// that exact fault, clears nothing and frees nothing; the owners stay
/// retained and the port does not retire.
#[test]
fn a_faulted_stopped_port_refuses_its_cleanup_with_its_fault_and_clears_nothing() {
    with_installations(|mut fixture, _| {
        let fixture = &mut fixture;
        fixture.start();
        let (replace, owners) = fixture.replacement();
        fixture.observed.borrow_mut().reject_key_writes = 2;
        assert!(matches!(
            fixture.run_failed(replace),
            Err(CommandFailed::Taken(_))
        ));
        assert!(fixture.port.recover_for_stop());
        fixture.stop();
        fixture.port.fault_for_test(Fault::Abandoned);
        let clears = fixture.observed.borrow().key_clears;
        for _ in 0..2 {
            assert_eq!(
                fixture.clean(),
                Err(CleanupRefused::Faulted(Fault::Abandoned)),
                "the exact cause"
            );
        }
        assert_eq!(
            fixture.observed.borrow().key_clears,
            clears,
            "nothing cleared"
        );
        assert!(fixture.group_terminals().is_empty(), "nothing freed");
        assert_eq!(
            fixture.port.retained_group().0,
            Some(owners),
            "the owners stay in place"
        );
        assert_eq!(fixture.port.fault(), Some(Fault::Abandoned));
        assert!(fixture.stopped_release_refused());
    });
}

#[test]
fn a_received_unit_keeps_its_bytes_and_owner_until_it_is_recycled() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.deliver(1);
        let now = fixture.now();
        let RxPull::Unit { frame, meta } = fixture.port.receive(8, now).expect("receive") else {
            panic!("the received MPDU");
        };
        assert_eq!(meta.common().rate, RxEvidence::Unavailable);
        assert_eq!(meta.common().rssi_dbm, RxEvidence::HardwareObserved(-42));
        assert_eq!(frame.bytes(), &crate::lower_mac::tests::data_frame(STATION));
        assert_eq!(count(&fixture.released), 0, "the caller holds the unit");
        assert!(fixture.port.recycle(frame).is_ok());
        assert_eq!(count(&fixture.released), 1, "recycled once");
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        assert!(port.release().is_some(), "nothing received is outstanding");
    });
}

#[test]
fn a_lent_unit_lends_its_storage_and_its_drop_returns_it_to_the_ring() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.deliver(1);
        let now = fixture.now();
        let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") else {
            panic!("the received MPDU");
        };
        let mut lent = fixture.port.lend(frame).ok().expect("lent");
        let mpdu = crate::lower_mac::tests::data_frame(STATION);
        assert_eq!(lent.mpdu_len(), mpdu.len());
        assert_eq!(lent.storage(), &mpdu);
        lent.storage_mut()[0] = 0xa5;
        assert_eq!(lent.storage()[0], 0xa5, "the network rewrites the unit");
        assert_eq!(count(&fixture.released), 0, "the network holds the unit");
        assert_eq!(fixture.port.lent(), 1);
        drop(lent);
        assert_eq!(count(&fixture.released), 1, "returned once");
        assert_eq!(fixture.port.lent(), 0);
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        assert!(port.release().is_some(), "the returned unit retires");
    });
}

#[test]
fn a_loss_past_the_held_count_settles_nothing_and_keeps_the_earlier_cause() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.deliver(1);
        let now = fixture.now();
        let RxPull::Unit { frame: kept, .. } = fixture.port.receive(8, now).expect("receive")
        else {
            panic!("a received unit");
        };
        assert_eq!(fixture.port.held_for_test(), 1);
        // An earlier cause a stop recovers from, then two losses where the
        // caller holds one unit: a count the held count excludes.
        let earlier = Fault::Unpublished(crate::port::Unpublished::Key);
        fixture.port.fault_for_test(earlier);
        fixture.port.lose_for_test(2);
        assert!(
            !fixture.port.recover_for_stop(),
            "no recovery follows a failed settlement"
        );
        assert_eq!(
            fixture.port.fault(),
            Some(earlier),
            "the earlier cause stays"
        );
        assert_eq!(fixture.port.held_for_test(), 1, "the held count stays");
        assert!(
            !fixture.port.recover_for_stop(),
            "the losses stay unsettled for the next look"
        );
        assert!(fixture.port.recycle(kept).is_ok());
    });
}

#[test]
fn a_loss_settles_exactly_the_units_it_read() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.deliver(2);
        let now = fixture.now();
        let RxPull::Unit { frame: lost, .. } = fixture.port.receive(8, now).expect("receive")
        else {
            panic!("a received unit");
        };
        let RxPull::Unit { frame: later, .. } = fixture.port.receive(8, now).expect("receive")
        else {
            panic!("a received unit");
        };
        drop(lost);
        assert!(fixture.port.recover_for_stop(), "the unit is back");
        assert_eq!(fixture.port.held_for_test(), 1, "one unit left the count");
        // A loss after the settlement is the port's again, not erased by it.
        drop(later);
        assert_eq!(fixture.port.fault(), Some(Fault::ReceiveLost));
        assert!(fixture.port.recover_for_stop());
        assert_eq!(fixture.port.held_for_test(), 0);
        assert_eq!(fixture.port.fault(), None);
    });
}

#[test]
fn a_dropped_received_buffer_is_a_receive_lost_fault_until_the_stop_and_its_unit_goes_back() {
    use oer_protocols_ieee80211_contracts_port::LendRefused;
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.deliver(3);
        let now = fixture.now();
        let RxPull::Unit { frame: lost, .. } = fixture.port.receive(8, now).expect("receive")
        else {
            panic!("a received unit");
        };
        let RxPull::Unit { frame: kept, .. } = fixture.port.receive(8, now).expect("receive")
        else {
            panic!("a received unit");
        };
        drop(lost);
        // The dropped buffer's unit goes back: nothing of it is forgotten.
        assert_eq!(count(&fixture.released), 1, "the dropped unit returned");
        let (pulls, returned) = {
            let observed = fixture.observed.borrow();
            (observed.pulls, observed.returned)
        };
        // Every later call answers with the loss.
        assert!(matches!(
            fixture.port.receive(8, now),
            Err(Fault::ReceiveLost)
        ));
        assert!(matches!(
            fixture
                .port
                .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox)
                .unwrap(),
            Err(Fault::ReceiveLost)
        ));
        let waker = fixture.waker();
        let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
        assert_eq!(facts.tx, Some(TxFacts::DUE), "the next service answers");
        let color = BssColor::new(5).expect("color");
        assert!(matches!(
            fixture
                .port
                .command(PortCommand::SetBssColor(color), now, &mut fixture.outbox)
                .unwrap(),
            Err(CommandFailed::Returned {
                fault: Fault::ReceiveLost,
                ..
            })
        ));
        let attempt = fixture.station_mpdu().expect("attempt");
        assert!(
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .unwrap()
                .is_err()
        );
        assert_eq!(
            fixture.take_completed(),
            1,
            "the refused attempt's terminal"
        );
        // The unit the caller still holds can only go back to the ring.
        let kept = match fixture.port.lend(kept) {
            Err(LendRefused { buffer, .. }) => buffer,
            Ok(_) => panic!("a faulted port lends nothing"),
        };
        assert!(fixture.port.recycle(kept).is_ok());
        assert_eq!(count(&fixture.released), 2, "the kept unit returned too");
        fixture.advance(COMMAND_BOUND);
        let now = fixture.now();
        assert!(matches!(
            fixture.port.receive(8, now),
            Err(Fault::ReceiveLost)
        ));
        assert!(matches!(
            fixture
                .port
                .poll_stop(&mut Context::from_waker(&waker), now),
            Poll::Ready(Err(StopUnconfirmed {
                cause: StopCause::Device(Fault::ReceiveLost),
                ..
            }))
        ));
        {
            let observed = fixture.observed.borrow();
            assert_eq!(observed.pulls, pulls, "the ring is not pulled again");
            assert_eq!(observed.returned, returned, "nothing rearmed the ring");
            assert_eq!(observed.received.len(), 1, "the third unit stays staged");
        }
        assert_eq!(count(&fixture.released), 2, "each unit returned once");
        // The stop's recovery takes the returned unit out of the held
        // count; the ordinary stop then confirms.
        assert!(fixture.port.recover_for_stop(), "the unit is back");
        assert_eq!(fixture.port.fault(), None);
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        assert!(port.release().is_some(), "the stopped port retires");
    });
}

#[test]
fn a_continuous_burst_reaches_the_caller_through_its_pulls_within_the_continuation_bound() {
    use crate::rx::receive::CONTINUATION_WAIT;
    const FRAMES: usize = 256;
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(FRAMES);
        fixture.observed.borrow_mut().continuous_rx = true;
        let begin = fixture.now();
        let mut taken = 0;
        for _ in 0..4 * FRAMES {
            if taken == FRAMES {
                break;
            }
            // The device model bounds the steps between its harness turns.
            fixture.observed.borrow_mut().driver_steps = 0;
            let pulls = fixture.observed.borrow().pulls;
            fixture.turn();
            assert_eq!(
                fixture.observed.borrow().pulls,
                pulls,
                "a transmit turn takes no unit"
            );
            let now = fixture.now();
            match fixture.port.receive(32, now).expect("receive") {
                RxPull::Unit { frame, .. } => {
                    assert!(fixture.port.recycle(frame).is_ok());
                    taken += 1;
                }
                RxPull::Later(at) => {
                    assert!(
                        at.saturating_duration_since(now) <= CONTINUATION_WAIT,
                        "the ring continues within its bound"
                    );
                    fixture.advance(at.saturating_duration_since(now));
                }
                RxPull::Empty => panic!("the ring holds the burst"),
            }
        }
        assert_eq!(taken, FRAMES);
        let observed = fixture.observed.borrow();
        assert!(observed.received.is_empty());
        assert_eq!(observed.rx_delivered, FRAMES);
        assert_eq!(count(&fixture.released), FRAMES, "each unit recycled once");
        // The model continues every fourth unit: the burst waits for those
        // continuations alone.
        let waited = fixture.now().saturating_duration_since(begin);
        assert!(waited.as_micros() <= (FRAMES / 4) as u64 * CONTINUATION_WAIT.as_micros());
    });
}

#[test]
fn lent_units_beyond_the_held_bound_reach_the_caller_through_facts_without_a_timer() {
    /// More units than the caller and the network hold at once.
    const LENT_FRAMES: usize = 64;
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(LENT_FRAMES);
        let begin = fixture.now();
        let waker = fixture.waker();
        for taken in 0..LENT_FRAMES {
            let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
            assert!(facts.rx, "unit {taken} is announced by a receive fact");
            let now = fixture.now();
            let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") else {
                panic!("unit {taken} reaches the caller");
            };
            // A transmit turn runs between the take and the lend.
            fixture.turn();
            let lent = fixture.port.lend(frame).ok().expect("the network takes it");
            drop(lent);
        }
        assert_eq!(fixture.now(), begin, "no unit waited for a timer");
        assert_eq!(count(&fixture.released), LENT_FRAMES);
        assert_eq!(fixture.port.lent(), 0);
        assert!(fixture.observed.borrow().received.is_empty());
    });
}

#[test]
fn the_network_leaves_the_pool_a_full_reorder_window_and_the_critical_reserve() {
    use crate::rx::transaction::DEFAULT_CRITICAL_RESERVE;
    for units in [RX_HELD + DEFAULT_CRITICAL_RESERVE + 1, 64, 96] {
        // The caller holds a window but its hole, the network all it may:
        // the hole's MPDU still finds a credit beyond the critical reserve.
        let outside = crate::port::lent_rx(units) + (RX_HELD - 1);
        assert!(units - outside > DEFAULT_CRITICAL_RESERVE, "{units} units");
    }
    assert_eq!(crate::port::lent_rx(RX_HELD), 0);
}

#[test]
fn idle_fact_polls_leave_the_ring_unpulled_until_it_holds_a_unit() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.configure_station();
        let waker = fixture.waker();
        let pulls = fixture.observed.borrow().pulls;
        for _ in 0..64 {
            let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
            assert!(!facts.rx, "the ring holds nothing");
        }
        assert_eq!(
            fixture.observed.borrow().pulls,
            pulls,
            "an idle poll reached the ring's pull"
        );
        let frame = fixture.frame();
        fixture.observed.borrow_mut().received.push_back(frame);
        let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
        assert!(facts.rx, "the staged unit is a receive fact");
        let now = fixture.now();
        let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") else {
            panic!("the announced unit");
        };
        assert!(fixture.port.recycle(frame).is_ok());
        assert_eq!(count(&fixture.released), 1);
    });
}

#[test]
fn each_set_edca_table_reaches_the_next_publications_contention_draw() {
    use oer_protocols_ieee80211_testing_conformance::edca::parameters;
    with_fixture(|fixture| {
        fixture.start();
        let tables = [
            parameters(VifRole::Station, false),
            EdcaParameterSet::STANDARD_DEFAULT,
        ];
        for (round, table) in tables.into_iter().enumerate() {
            let edca = SupportedEdca::new(table).expect("EDCA");
            assert!(matches!(
                fixture.run(PortCommand::SetEdca(edca)),
                Ok(CommandProgress::Done)
            ));
            let attempt = fixture.station_mpdu().expect("attempt");
            let now = fixture.now();
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .unwrap()
                .expect("submitted");
            for _ in 0..4 {
                fixture.turn();
            }
            assert_eq!(fixture.take_completed(), 1, "round {round}");
            let observed = fixture.observed.borrow();
            let draws = observed.draws.borrow();
            assert_eq!(draws.len(), round + 1, "one draw per publication");
            let draw = *draws.last().expect("the publication's draw");
            assert_eq!(draw.role, VifRole::Station);
            assert_eq!(draw.category, WmmAccessCategory::BestEffort);
            assert_eq!(
                draw.parameters,
                table.access_category(WmmAccessCategory::BestEffort),
                "round {round}: the draw took the table SetEdca gave"
            );
            assert_eq!(
                observed.aifsns.last(),
                Some(&draw.parameters.wmm.aifsn.get()),
                "the program waits the drawn AIFSN"
            );
            assert_eq!(
                observed.backoffs.last(),
                Some(&draw.backoff_slots),
                "the program counts the drawn backoff"
            );
        }
    });
}

#[test]
fn a_set_edca_lifetime_expires_a_queued_mpdu_at_its_exact_deadline() {
    with_fixture(|fixture| {
        fixture.start();
        let lifetime = Duration::from_micros(100);
        let table = EdcaParameterSet::new(EdcaParameterSet::STANDARD_DEFAULT.wmm(), [lifetime; 4]);
        let edca = SupportedEdca::new(table).expect("a sub-TU lifetime");
        assert!(matches!(
            fixture.run(PortCommand::SetEdca(edca)),
            Ok(CommandProgress::Done)
        ));
        let gate = fixture.gate.take().expect("gate");
        assert!(matches!(
            fixture.run(PortCommand::CloseGate(gate)),
            Ok(CommandProgress::Done)
        ));
        let idle = fixture.port.deadline();
        let attempt = fixture.station_mpdu().expect("attempt");
        let admitted = fixture.now();
        fixture
            .port
            .submit(attempt, admitted, &mut fixture.outbox)
            .unwrap()
            .expect("queued");
        let expiry = admitted.saturating_add(lifetime);
        assert_eq!(
            fixture.port.deadline(),
            Some(idle.map_or(expiry, |idle| idle.min(expiry)))
        );
        fixture.advance(Duration::from_micros(99));
        fixture.turn();
        assert_eq!(fixture.take_completed(), 0, "alive before its lifetime");
        fixture.advance(Duration::from_micros(1));
        assert_eq!(fixture.now(), expiry);
        fixture.turn();
        assert_eq!(fixture.take_completed(), 1, "expired at its deadline");
        assert_eq!(fixture.port.expired_msdus(), 1);
        assert_eq!(fixture.observed.borrow().publications, 0, "never published");
    });
}

#[test]
fn a_unit_a_cut_drains_past_the_held_units_goes_back_to_the_ring_beside_the_fault() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.configure_station();
        fixture.deliver(RX_HELD);
        let now = fixture.now();
        let mut held = Vec::new();
        while held.len() < RX_HELD {
            let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") else {
                panic!("a received unit");
            };
            held.push(frame);
        }
        let frame = fixture.frame();
        fixture.observed.borrow_mut().captured.push_back(frame);
        let released = count(&fixture.released);
        let (association, slot, _) = fixture.association();
        let permission = fixture.group_permission.take().expect("permission");
        let install =
            fixture
                .commands
                .install_group(&association, permission, slot, key_id(1), group_key(1));
        let _ran = fixture.run(install);
        assert_eq!(fixture.port.fault(), Some(Fault::ReceiveOverflow));
        assert_eq!(
            count(&fixture.released),
            released + 1,
            "the drained unit went back to the ring"
        );
        assert!(fixture.outbox.received.is_empty(), "the caller got none");
        for frame in held {
            assert!(fixture.port.recycle(frame).is_ok());
        }
    });
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// Run `command` until it leaves its barrier, serving a turn between
    /// holds: its progress or its failure, as the port returned them.
    fn run_failed(
        &mut self,
        mut command: Command<'id, Id>,
    ) -> Result<CommandProgress<'id, Family<Id>, Caps>, CommandFailed<Command<'id, Id>, Fault>>
    {
        for _ in 0..16 {
            let now = self.now();
            match self.port.command(command, now, &mut self.outbox).unwrap()? {
                CommandProgress::Held(held) => command = held,
                other => return Ok(other),
            }
            self.turn();
        }
        Ok(CommandProgress::Held(command))
    }

    /// The slot and permission a returned station group command holds.
    fn group_owners(command: &Command<'id, Id>) -> Option<(u8, u8)> {
        match command {
            PortCommand::InstallGroup(install) => Some(install.owners_for_test()),
            PortCommand::SelectGroup {
                permission,
                installed,
            } => Some((
                <Keys<Id> as KeyTypes<'id>>::group_slot_identity(installed),
                permission.index(),
            )),
            _ => None,
        }
    }
}

/// A pairwise installation the hardware rejects comes back whole, its
/// slot in it, beside the fault: the core did not take the slot.
#[test]
fn a_pairwise_installation_the_hardware_rejects_comes_back_with_its_slot() {
    with_fixture(|fixture| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        fixture.observed.borrow_mut().reject_key_writes = 1;
        match fixture.run_failed(install) {
            Err(CommandFailed::Returned {
                command: PortCommand::InstallPairwise(_),
                fault,
            }) => assert_eq!(fault, KEY_REJECTED),
            _ => panic!("the installation comes back beside its fault"),
        }
        assert!(fixture.outbox.terminals.is_empty(), "no terminal");
    });
}

/// A group installation the hardware rejects after its receive cut comes
/// back whole, its slot and permission in it, beside the fault; the cut
/// resumes.
#[test]
fn a_group_installation_the_hardware_rejects_comes_back_with_its_slot_and_permission() {
    with_fixture(|fixture| {
        fixture.start();
        let (install, owners) = fixture.group_command(Mutation::Install);
        let cuts = fixture.observed.borrow().resumed_cuts;
        fixture.observed.borrow_mut().reject_key_writes = 1;
        let returned = match fixture.run_failed(install) {
            Err(CommandFailed::Returned {
                command: command @ PortCommand::InstallGroup(_),
                fault,
            }) => {
                assert_eq!(fault, KEY_REJECTED);
                command
            }
            _ => panic!("the installation comes back beside its fault"),
        };
        assert_eq!(fixture.observed.borrow().resumed_cuts, cuts + 1);
        assert!(fixture.group_terminals().is_empty(), "no terminal");
        // The very slot and permission the command took.
        assert_eq!(Fixture::group_owners(&returned), Some(owners));
    });
}

/// A group selection whose key the driver no longer holds comes back
/// whole beside the fault, its key and permission in it, before the
/// driver takes anything; the cut resumes.
#[test]
fn a_group_selection_of_a_missing_key_comes_back_with_its_owners() {
    with_fixture(|fixture| {
        fixture.start();
        let (select, owners) = fixture.group_command(Mutation::Select);
        fixture.port.uninstall_keys_for_test();
        let cuts = fixture.observed.borrow().resumed_cuts;
        match fixture.run_failed(select) {
            Err(CommandFailed::Returned {
                command: command @ PortCommand::SelectGroup { .. },
                fault: Fault::KeySlotMissing(crate::port::KeyOp::Select),
            }) => assert_eq!(Fixture::group_owners(&command), Some(owners)),
            _ => panic!("the selection comes back beside its fault"),
        }
        assert_eq!(fixture.observed.borrow().resumed_cuts, cuts + 1);
    });
}
