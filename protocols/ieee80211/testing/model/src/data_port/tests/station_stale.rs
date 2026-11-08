//! The station's tokens of an earlier installation on the model's port,
//! through the public contract: after a reinstallation of the same
//! identity each one compiles and is refused at its acceptance point, with
//! every owner back. Each case names exactly one earlier token per
//! operation, so the refusal is that token's.

use super::access_point::{End, Ended, StaPairwise, reinstall, settle, station_frame, submit};
use super::*;
use oer_protocols_ieee80211_contracts_port::{InstalledGroup, InstalledPairwise};
use oer_protocols_ieee80211_wire::security::Spp;

type Command<'s> = PortCommand<'s, Types<'s>, ModelCaps>;
type StaGroup<'s> = ModelGroupKey<'s, B<'s>, Station>;
type StaGroupPermission<'s> = oer_protocols_ieee80211_contracts_port::IssuedSlot<
    's,
    ModelGroupPermission<Station>,
    ModelPool<LocalId<'s>, 8>,
>;

/// The command a refusal for a token of installation 0 on installation 1
/// returned.
fn refused_stale<'s>(progress: CommandProgress<'s, Types<'s>, ModelCaps>) -> Command<'s> {
    match progress {
        CommandProgress::Refused(
            command,
            CommandRefusal::StalePermission {
                issued: 0,
                installation: 1,
            },
        ) => command,
        _ => panic!("a token of the earlier installation is refused"),
    }
}

/// Install the station's pairwise key in `slot` and its group key in
/// `group_slot` under `association`, and take both from their terminals.
fn install_keys<'s>(
    harness: &mut Harness<'s>,
    association: &ModelAssociation<'s, B<'s>, Station>,
    slot: PairwiseKeySlot<'s, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<LocalId<'s>, 5>>,
    group_slot: GroupKeySlot<
        's,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<LocalId<'s>, 5>,
    >,
    permission: StaGroupPermission<'s>,
) -> (StaPairwise<'s>, StaGroup<'s>, StaGroupPermission<'s>) {
    let (install, _) = harness.commands.install_pairwise(
        association,
        slot,
        IndividualAddress::new(PEER).unwrap(),
        Ccmp128Key::new(&[5; 16]),
    );
    assert!(matches!(harness.command(install), CommandProgress::Done));
    let install = harness.commands.install_group(
        association,
        permission,
        group_slot,
        GroupKeyId::new(1).unwrap(),
        Ccmp128Key::new(&[6; 16]),
    );
    assert!(matches!(harness.command(install), CommandProgress::Done));
    let mut installed = core::mem::take(&mut harness.sink.terminals).into_iter();
    let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(pairwise))) =
        installed.next()
    else {
        panic!("the station's pairwise terminal");
    };
    let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission })) =
        installed.next()
    else {
        panic!("the station's group terminal");
    };
    (pairwise, key, permission)
}

#[test]
fn a_station_configure_permission_of_an_earlier_installation_is_refused_with_it() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let stale = harness.configure().unwrap();
        let PortCommand::Configure { permission, .. } = refused_stale(harness.command(stale))
        else {
            panic!("the configuration comes back");
        };
        assert_eq!(permission.generation(), 0, "the permission comes back");
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        // The current installation's permission configures.
        let current = PortCommand::Configure {
            permission: resources.station.commands.configure,
            receive: StationReceive::Off,
        };
        assert!(matches!(harness.command(current), CommandProgress::Done));
        assert!(harness.take_configured());
        assert_eq!(harness.configure.as_ref().unwrap().generation(), 1);
    }));
}

#[test]
fn an_earlier_installations_token_is_refused_for_good_even_while_the_radio_is_stopped() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (mut harness, _resources) = reinstall(harness);
        // Not started: a current token is refused as `NotEnabled`, which a
        // start answers; an earlier one is refused as stale, which none
        // answers.
        let stale = harness.configure().unwrap();
        let _command = refused_stale(harness.command(stale));
    }));
}

#[test]
fn every_station_key_installation_with_an_earlier_token_is_refused_with_its_owners() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (stale_start, stale_group, stale_permission, stale_pairwise) =
            harness.group.take().unwrap();
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let current = resources.station;
        let stale_association =
            ModelKeys::<B<'_>, Station>::begin_association(stale_start, Spp::NotNegotiated);
        let association =
            ModelKeys::<B<'_>, Station>::begin_association(current.association, Spp::NotNegotiated);
        let [stale_pairwise, _] = stale_pairwise;
        let [pairwise, _] = current.pairwise;
        let [stale_group, _] = stale_group;
        let [group, _] = current.group;
        let peer = IndividualAddress::new(PEER).unwrap();
        // An earlier slot under the current association.
        let (install, _) = harness.commands.install_pairwise(
            &association,
            stale_pairwise,
            peer,
            Ccmp128Key::new(&[5; 16]),
        );
        assert!(matches!(
            refused_stale(harness.command(install)),
            PortCommand::InstallPairwise(_)
        ));
        // A current slot under the earlier association.
        let (install, _) = harness.commands.install_pairwise(
            &stale_association,
            pairwise,
            peer,
            Ccmp128Key::new(&[5; 16]),
        );
        assert!(matches!(
            refused_stale(harness.command(install)),
            PortCommand::InstallPairwise(_)
        ));
        // The earlier group permission with a current slot, the current
        // permission with an earlier slot.
        let install = harness.commands.install_group(
            &association,
            stale_permission,
            group,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[6; 16]),
        );
        assert!(matches!(
            refused_stale(harness.command(install)),
            PortCommand::InstallGroup(_)
        ));
        let install = harness.commands.install_group(
            &association,
            current.group_permission,
            stale_group,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[6; 16]),
        );
        assert!(matches!(
            refused_stale(harness.command(install)),
            PortCommand::InstallGroup(_)
        ));
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        assert_eq!(harness.air.group_writes(), 0, "no key was written");
    }));
}

#[test]
fn every_station_key_change_naming_an_earlier_token_is_refused_with_its_owners() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (start, [group_slot, _], permission, [slot, _]) = harness.group.take().unwrap();
        let stale_association =
            ModelKeys::<B<'_>, Station>::begin_association(start, Spp::NotNegotiated);
        let (stale_pairwise, stale_group, stale_permission) = install_keys(
            &mut harness,
            &stale_association,
            slot,
            group_slot,
            permission,
        );
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let current = resources.station;
        let association =
            ModelKeys::<B<'_>, Station>::begin_association(current.association, Spp::NotNegotiated);
        let [slot, _] = current.pairwise;
        let [group_slot, _] = current.group;
        let (pairwise, group, permission) = install_keys(
            &mut harness,
            &association,
            slot,
            group_slot,
            current.group_permission,
        );
        // Each command names one earlier token.
        let PortCommand::RemovePairwise {
            association: stale_association,
            key: pairwise,
        } = refused_stale(harness.command(PortCommand::RemovePairwise {
            association: stale_association,
            key: pairwise,
        }))
        else {
            panic!("an earlier association does not remove a current key");
        };
        let PortCommand::RemovePairwise {
            association,
            key: stale_pairwise,
        } = refused_stale(harness.command(PortCommand::RemovePairwise {
            association,
            key: stale_pairwise,
        }))
        else {
            panic!("an earlier key is not removed under the current association");
        };
        let PortCommand::RemoveGroup {
            permission: stale_permission,
            key: group,
        } = refused_stale(harness.command(PortCommand::RemoveGroup {
            permission: stale_permission,
            key: group,
        }))
        else {
            panic!("an earlier permission does not remove a current group key");
        };
        let PortCommand::RemoveGroup {
            permission,
            key: stale_group,
        } = refused_stale(harness.command(PortCommand::RemoveGroup {
            permission,
            key: stale_group,
        }))
        else {
            panic!("an earlier group key is not removed with the current permission");
        };
        let PortCommand::SelectGroup {
            permission,
            installed: stale_group,
        } = refused_stale(harness.command(PortCommand::SelectGroup {
            permission,
            installed: stale_group,
        }))
        else {
            panic!("an earlier group key is not selected");
        };
        let PortCommand::ReplaceGroup {
            permission: stale_permission,
            installed: group,
            ..
        } = refused_stale(harness.command(PortCommand::ReplaceGroup {
            permission: stale_permission,
            installed: group,
            key_id: GroupKeyId::new(2).unwrap(),
            key: Ccmp128Key::new(&[7; 16]),
        }))
        else {
            panic!("an earlier permission does not replace a current group key");
        };
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        assert_eq!(
            (
                stale_association.owner.generation(),
                stale_permission.generation()
            ),
            (0, 0)
        );
        // The current tokens still change the current keys.
        assert!(matches!(
            harness.command(PortCommand::RemoveGroup {
                permission,
                key: group,
            }),
            CommandProgress::Done
        ));
        assert!(matches!(
            harness.command(PortCommand::RemovePairwise {
                association,
                key: pairwise,
            }),
            CommandProgress::Done
        ));
        assert_eq!(harness.sink.terminals.len(), 2, "both removals end");
        drop((
            stale_association,
            stale_permission,
            stale_pairwise,
            stale_group,
        ));
    }));
}

#[test]
fn a_station_frame_under_a_key_of_an_earlier_installation_is_refused_at_submit() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (start, [group_slot, _], permission, [slot, _]) = harness.group.take().unwrap();
        let association = ModelKeys::<B<'_>, Station>::begin_association(start, Spp::NotNegotiated);
        let (key, _group, _permission) =
            install_keys(&mut harness, &association, slot, group_slot, permission);
        // The frame is prepared and sealed under the first installation.
        let sealed = station_frame(&mut harness, &key, 1);
        let ticket = sealed.operation.ticket().index();
        let (mut harness, _resources) = reinstall(harness);
        harness.start();
        let now = harness.clock.now();
        assert_eq!(
            harness.port.submit(sealed, now, &mut harness.sink),
            Err(SubmitRefusal::StalePermission {
                issued: 0,
                installation: 1,
            })
        );
        let mut beacon = None;
        assert_eq!(
            settle(&mut harness, &mut beacon),
            [Ended::Frame(
                ticket,
                End::Withdrawn(WithdrawCause::StalePermission)
            )],
            "its terminal returns ticket, buffer and context"
        );
        assert!(harness.air.sent().is_empty(), "nothing went on the air");
        // An open frame of the current installation goes.
        let current = harness.mpdu().unwrap();
        let current = submit(&mut harness, current);
        harness.service();
        assert_eq!(
            settle(&mut harness, &mut beacon),
            [Ended::Frame(current, End::Delivered)]
        );
        drop(key);
    }));
}

#[test]
fn a_frame_under_an_earlier_key_is_refused_as_stale_while_the_radio_is_stopped() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (start, [group_slot, _], permission, [slot, _]) = harness.group.take().unwrap();
        let association = ModelKeys::<B<'_>, Station>::begin_association(start, Spp::NotNegotiated);
        let (key, _group, _permission) =
            install_keys(&mut harness, &association, slot, group_slot, permission);
        let (mut harness, resources) = reinstall(harness);
        // Not started. The current installation's ticket and buffer, the
        // earlier installation's key: the stale refusal comes first, as no
        // start answers it.
        let mut tickets = resources.shared.tickets;
        let mut small = resources.shared.small;
        let ticket = tickets.pop().unwrap();
        let buffer = small.pop().unwrap();
        let attempt = harness.admitted_under_number(ticket, buffer, &key, 1);
        let ticket = attempt.operation.ticket().index();
        let now = harness.clock.now();
        assert_eq!(
            harness.port.submit(attempt, now, &mut harness.sink),
            Err(SubmitRefusal::StalePermission {
                issued: 0,
                installation: 1,
            })
        );
        harness.tickets = tickets;
        harness.small = small;
        let mut beacon = None;
        assert_eq!(
            settle(&mut harness, &mut beacon),
            [Ended::Frame(
                ticket,
                End::Withdrawn(WithdrawCause::StalePermission)
            )]
        );
    }));
}
