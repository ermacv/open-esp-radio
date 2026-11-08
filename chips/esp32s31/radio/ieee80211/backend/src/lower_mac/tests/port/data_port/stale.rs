//! The S31 data port refuses the station's tokens of an earlier
//! installation: after a reinstallation each one compiles and is refused at
//! its acceptance point, `command` or `submit`, with every owner back. Each
//! operation names exactly one earlier token.
use super::lifecycle::reinstall;
use super::*;

type Keys<Id> = PortKeys<Id, Station>;
type Command<'id, Id> = PortCommand<'id, Family<Id>, Caps>;

/// The command a refusal for a token of installation 0 on installation 1
/// returned.
fn refused_stale<'id, Id: radio::SlotIdentity + 'id>(
    progress: Result<CommandProgress<'id, Family<Id>, Caps>, Fault>,
) -> Command<'id, Id> {
    match progress {
        Ok(CommandProgress::Refused(
            command,
            CommandRefusal::StalePermission {
                issued: 0,
                installation: 1,
            },
        )) => command,
        _ => panic!("a token of the earlier installation is refused"),
    }
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// Install the station's pairwise and group keys on its own tokens.
    fn installed_keys(
        &mut self,
    ) -> (
        <Keys<Id> as KeyTypes<'id>>::Association,
        <Keys<Id> as KeyTypes<'id>>::PairwiseKey,
        <Keys<Id> as KeyTypes<'id>>::GroupKey,
        <Keys<Id> as KeyTypes<'id>>::GroupPermission,
    ) {
        let (association, group_slot, slot) = self.association();
        let (install, _) = self.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        assert!(matches!(self.run(install), Ok(CommandProgress::Done)));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(pairwise))) =
            self.outbox.terminals.pop()
        else {
            panic!("the pairwise terminal");
        };
        let permission = self.group_permission.take().expect("permission");
        let install = self.commands.install_group(
            &association,
            permission,
            group_slot,
            key_id(1),
            group_key(1),
        );
        assert!(matches!(self.run(install), Ok(CommandProgress::Done)));
        let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission })) =
            self.outbox.terminals.pop()
        else {
            panic!("the group terminal");
        };
        (association, pairwise, key, permission)
    }
}

#[test]
fn a_station_configure_permission_of_an_earlier_installation_is_refused_with_it() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let stale = fixture.configure.take().expect("permission");
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        // Refused for good before the radio runs, and after.
        let PortCommand::Configure {
            permission: stale, ..
        } = refused_stale(fixture.run(PortCommand::Configure {
            permission: stale,
            receive: StationReceive::Off,
        }))
        else {
            panic!("the configuration comes back");
        };
        fixture.start();
        let PortCommand::Configure { permission, .. } =
            refused_stale(fixture.run(PortCommand::Configure {
                permission: stale,
                receive: StationReceive::Off,
            }))
        else {
            panic!("the configuration comes back");
        };
        assert_eq!(permission.generation(), 0);
        assert!(fixture.outbox.terminals.is_empty(), "no terminal is owed");
        // The current installation's permission configures.
        let current = fixture.configure().expect("permission");
        assert!(matches!(fixture.run(current), Ok(CommandProgress::Done)));
    });
}

#[test]
fn a_station_key_installation_with_an_earlier_token_is_refused_with_its_owners() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let (stale_association, stale_group_slot, stale_slot) = fixture.association();
        let stale_permission = fixture.group_permission.take().expect("permission");
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        fixture.start();
        let (association, group_slot, slot) = fixture.association();
        let peer = IndividualAddress::new(BSSID).expect("peer");
        let (install, _) =
            fixture
                .commands
                .install_pairwise(&association, stale_slot, peer, group_key(7));
        assert!(matches!(
            refused_stale(fixture.run(install)),
            PortCommand::InstallPairwise(_)
        ));
        let (install, _) =
            fixture
                .commands
                .install_pairwise(&stale_association, slot, peer, group_key(7));
        assert!(matches!(
            refused_stale(fixture.run(install)),
            PortCommand::InstallPairwise(_)
        ));
        let install = fixture.commands.install_group(
            &association,
            stale_permission,
            group_slot,
            key_id(1),
            group_key(1),
        );
        assert!(matches!(
            refused_stale(fixture.run(install)),
            PortCommand::InstallGroup(_)
        ));
        let permission = fixture.group_permission.take().expect("permission");
        let install = fixture.commands.install_group(
            &association,
            permission,
            stale_group_slot,
            key_id(1),
            group_key(1),
        );
        assert!(matches!(
            refused_stale(fixture.run(install)),
            PortCommand::InstallGroup(_)
        ));
        assert!(fixture.outbox.terminals.is_empty(), "no terminal is owed");
    });
}

#[test]
fn a_change_of_a_key_an_earlier_installation_installed_is_refused_with_its_owners() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        // The keys stay installed: the new installation installs none, so
        // each command names exactly the earlier key.
        let (_stale_association, stale_pairwise, stale_group, _stale_permission) =
            fixture.installed_keys();
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        fixture.start();
        let (association, _group_slot, _slot) = fixture.association();
        let permission = fixture.group_permission.take().expect("permission");
        let PortCommand::RemovePairwise { association, .. } =
            refused_stale(fixture.run(PortCommand::RemovePairwise {
                association,
                key: stale_pairwise,
            }))
        else {
            panic!("an earlier key is not removed under the current association");
        };
        let PortCommand::RemoveGroup {
            permission,
            key: stale_group,
        } = refused_stale(fixture.run(PortCommand::RemoveGroup {
            permission,
            key: stale_group,
        }))
        else {
            panic!("an earlier group key is not removed with the current permission");
        };
        let PortCommand::SelectGroup { permission, .. } =
            refused_stale(fixture.run(PortCommand::SelectGroup {
                permission,
                installed: stale_group,
            }))
        else {
            panic!("an earlier group key is not selected");
        };
        assert!(fixture.outbox.terminals.is_empty(), "no terminal is owed");
        assert_eq!(
            (association.generation(), permission.generation()),
            (1, 1),
            "the current owners come back"
        );
    });
}

#[test]
fn an_earlier_association_or_group_permission_changes_no_current_key() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        // The first installation removes its keys: their slots, its
        // association and its group permission come back to the caller.
        let (association, pairwise, group, permission) = fixture.installed_keys();
        assert!(matches!(
            fixture.run(PortCommand::RemoveGroup {
                permission,
                key: group,
            }),
            Ok(CommandProgress::Done)
        ));
        assert!(matches!(
            fixture.run(PortCommand::RemovePairwise {
                association,
                key: pairwise,
            }),
            Ok(CommandProgress::Done)
        ));
        let mut removed = core::mem::take(&mut fixture.outbox.terminals).into_iter();
        let Some(LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
            permission: stale_permission,
            ..
        }))) = removed.find(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { .. }))
            )
        })
        else {
            panic!("the group removal's terminal");
        };
        let Some(LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise {
            association: stale_association,
            ..
        }))) = removed.find(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise { .. }))
            )
        })
        else {
            panic!("the pairwise removal's terminal");
        };
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        fixture.start();
        let (association, pairwise, group, permission) = fixture.installed_keys();
        let PortCommand::RemovePairwise {
            association: stale_association,
            key: pairwise,
        } = refused_stale(fixture.run(PortCommand::RemovePairwise {
            association: stale_association,
            key: pairwise,
        }))
        else {
            panic!("an earlier association does not remove a current key");
        };
        let PortCommand::RemoveGroup {
            permission: stale_permission,
            key: group,
        } = refused_stale(fixture.run(PortCommand::RemoveGroup {
            permission: stale_permission,
            key: group,
        }))
        else {
            panic!("an earlier permission does not remove a current group key");
        };
        let PortCommand::ReplaceGroup {
            permission: stale_permission,
            installed: group,
            ..
        } = refused_stale(fixture.run(PortCommand::ReplaceGroup {
            permission: stale_permission,
            installed: group,
            key_id: key_id(2),
            key: group_key(2),
        }))
        else {
            panic!("an earlier permission does not replace a current group key");
        };
        assert!(fixture.outbox.terminals.is_empty(), "no terminal is owed");
        assert_eq!(
            (
                stale_association.generation(),
                stale_permission.generation()
            ),
            (0, 0)
        );
        // The current tokens still change the current keys.
        assert!(matches!(
            fixture.run(PortCommand::RemoveGroup {
                permission,
                key: group,
            }),
            Ok(CommandProgress::Done)
        ));
        assert!(matches!(
            fixture.run(PortCommand::RemovePairwise {
                association,
                key: pairwise,
            }),
            Ok(CommandProgress::Done)
        ));
    });
}

#[test]
fn a_station_frame_under_a_key_of_an_earlier_installation_is_refused_at_submit() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let (_association, key, _group, _permission) = fixture.installed_keys();
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        fixture.start();
        // A frame of the current installation's ticket and buffer, under
        // the earlier installation's key.
        let sealed = fixture.protected_mpdu(&key).expect("a frame");
        let published = fixture.observed.borrow().publications;
        let now = fixture.now();
        assert_eq!(
            fixture.port.submit(sealed, now, &mut fixture.outbox),
            Err(SubmitRefusal::StalePermission {
                issued: 0,
                installation: 1,
            })
        );
        match fixture.outbox.terminals.as_slice() {
            [LowerMacTerminal::TxCompleted(TxCompleted::Single(completed))] => assert!(matches!(
                completed.outcome(),
                TxOutcome::Withdrawn(WithdrawCause::StalePermission)
            )),
            _ => panic!("its withdrawn terminal returns its owners"),
        }
        assert_eq!(
            fixture.observed.borrow().publications,
            published,
            "nothing published"
        );
        drop(key);
    });
}
