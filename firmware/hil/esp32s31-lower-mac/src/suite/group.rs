//! Actual GTK command progress, including terminals drained by disable.
use super::*;
use oer_hil_protocol::lower_mac::{Trace, group_trace};
use oer_ieee80211_port::conformance::group_keys::{
    Completion, Mutation, Owners, ProgressCase, ProgressTrace, Timing,
};
use oer_ieee80211_wire::{
    ccmp::{Ccmp128Key, GroupKeyId},
    security::Spp,
};
use oer_time::Clock as _;
type Permission<'id, Id> = <PortKeys<Id, Station> as KeyTypes<'id>>::GroupPermission;
type Events = heapless::Vec<Completion, 4>;
type Retained = heapless::Vec<Owners, 4>;
mod published;
async fn installed<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
) -> Result<(GroupKey<'id, Id, Station>, Permission<'id, Id>), &'static str> {
    loop {
        match phase.next_event().await {
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::Station { key, permission },
            )))) => return Ok((key, permission)),
            Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
            Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
            _ => return Err("station GTK setup"),
        }
    }
}
fn observed<'id, Id: radio::SlotIdentity + 'id>(
    terminal: LowerMacTerminal<'id, Family<Id>>,
) -> Result<Completion, &'static str> {
    let (mutation, slot, permission) = match terminal {
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }) => (
            Mutation::Install,
            PortKeys::<Id, Station>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::GroupKeyReplaced { key, permission } => (
            Mutation::Replace,
            PortKeys::<Id, Station>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::GroupKeySelected { key, permission } => (
            Mutation::Select,
            PortKeys::<Id, Station>::group_slot_identity(&key),
            permission.index(),
        ),
        LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { slot, permission })) => {
            (Mutation::Remove, slot.index(), permission.index())
        }
        LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
            slot,
            permission,
        })) => (Mutation::Remove, slot.index(), permission.index()),
        _ => return Err("unexpected group terminal"),
    };
    Ok(Completion::Group {
        mutation,
        owners: Owners { slot, permission },
    })
}
async fn admit<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    mutation: Mutation,
) -> Result<(Owners, Timing), &'static str> {
    let (start, bank) = resources.station_association.into_parts();
    let (_, [slot]) = bank.into_parts();
    let association = start.begin(Spp::NotNegotiated);
    let id = GroupKeyId::new(1).ok_or("KeyID")?;
    let (key, permission, vacant) = if mutation == Mutation::Install {
        (None, resources.station_group_permission, Some(slot))
    } else {
        phase.station().install_group_key(
            &association,
            resources.station_group_permission,
            slot,
            id,
            Ccmp128Key::new(&[1; 16]),
        );
        let (key, permission) = installed(phase).await?;
        (Some(key), permission, None)
    };
    let owners = Owners {
        slot: match &key {
            Some(k) => PortKeys::<Id, Station>::group_slot_identity(k),
            None => vacant.as_ref().ok_or("vacant GTK")?.index(),
        },
        permission: permission.index(),
    };
    oer_espressif_staged_runtime::trace::start(port::trace::Publication::CHANNEL.mask());
    let now = EmbassyClock.now();
    match mutation {
        Mutation::Install => phase.station().install_group_key(
            &association,
            permission,
            vacant.ok_or("install slot")?,
            id,
            Ccmp128Key::new(&[2; 16]),
        ),
        Mutation::Replace => phase.station().replace_group_key(
            permission,
            key.ok_or("replace key")?,
            GroupKeyId::new(2).ok_or("replacement KeyID")?,
            Ccmp128Key::new(&[2; 16]),
        ),
        Mutation::Select => phase
            .station()
            .select_group_key(permission, key.ok_or("select key")?),
        Mutation::Remove => phase
            .station()
            .remove_group_key(permission, key.ok_or("remove key")?),
    }
    Ok((
        owners,
        Timing {
            admitted_at: now,
            completed_at: now,
        },
    ))
}
pub(super) async fn run<'p, 'id, Id: radio::SlotIdentity + 'id>(
    mut owner: Owner<'p, 'id, Id, Backend<Id>, radio::Enabled>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    case: ProgressCase,
    peer: Peer,
) -> Ended<Id, Backend<Id>, Result<Trace, &'static str>> {
    if case == ProgressCase::PublishedRemoval {
        return published::run(owner, resources, peer).await;
    }
    if case == ProgressCase::ReplacementRollback {
        let result = async {
            let mut phase = owner.phase();
            phase.debug_reject_group_replacement();
            let (admitted, mut timing) = admit(&mut phase, resources, Mutation::Replace).await?;
            let poisoned = loop {
                match phase.next_event().await {
                    Err(radio::EventEnd::Quarantined { .. }) => break true,
                    Err(_) => return Err("rollback event stream ended without quarantine"),
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                    Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
                    _ => return Err("rollback emitted an ordinary terminal"),
                }
            };
            let (observed, completed) = TRACE
                .records()
                .find_map(|row| {
                    row.decode::<port::trace::GroupRollback>()
                        .map(|event| (event, row.t_us))
                })
                .ok_or("missing hardware rollback observation")?;
            let elapsed = completed.wrapping_sub(timing.admitted_at.as_micros() as u32);
            timing.completed_at = timing
                .admitted_at
                .saturating_add(Duration::from_micros(u64::from(elapsed)));
            let mut retained = Retained::new();
            retained
                .push(Owners {
                    slot: observed.owners[0],
                    permission: observed.owners[1],
                })
                .map_err(|_| "rollback custody extent")?;
            let raw = ProgressTrace {
                rollback: Some(oer_ieee80211_port::conformance::group_keys::Rollback {
                    previous_key_id: observed.previous_key_id,
                    attempted_key_id: observed.attempted_key_id,
                    restored: observed.restored,
                }),
                admitted,
                timing,
                retained,
                events: Events::new(),
                poisoned,
                deadline_woke: false,
                published: None,
                blocked_while_published: false,
            };
            group_trace(&raw, Backend::<Id>::GROUP_COMMAND_BOUND.as_micros())
                .map(Trace::Group)
                .ok_or("rollback trace extent")
        }
        .await;
        return match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Err(failed) => Ended::fail_stop(failed.fail_stop(), result),
            Ok(owner) => Ended::uninstall(owner, (), Err("rollback did not fail-stop")),
        };
    }
    let result = match case {
        ProgressCase::Disable { mutation, applied } => {
            let setup = admit(&mut owner.phase(), resources, mutation).await;
            if applied && setup.is_ok() {
                while !TRACE
                    .records()
                    .any(|row| row.decode::<port::trace::TerminalPosted>().is_some())
                {
                    barrier::progress().await;
                }
            }
            setup
        }
        _ => Err("healthy group disable case required"),
    };
    let mut events = Events::new();
    let mut completed_at = EmbassyClock.now();
    let mut invalid = false;
    let disabled = owner
        .disable(&mut |terminal| {
            match observed(terminal) {
                Ok(event) => {
                    if events.push(event).is_err() {
                        invalid = true;
                    }
                }
                Err(_) => invalid = true,
            }
            completed_at = EmbassyClock.now();
            core::ops::ControlFlow::Continue(())
        })
        .await;
    let result = result.and_then(|(admitted, mut timing)| {
        if invalid {
            return Err("group terminal identity");
        }
        timing.completed_at = completed_at;
        let raw = ProgressTrace {
            rollback: None,
            admitted,
            timing,
            events,
            retained: Retained::new(),
            poisoned: disabled.is_err(),
            deadline_woke: false,
            published: None,
            blocked_while_published: false,
        };
        group_trace(&raw, Backend::<Id>::GROUP_COMMAND_BOUND.as_micros())
            .map(Trace::Group)
            .ok_or("group trace extent")
    });
    match disabled {
        Ok(owner) => Ended::uninstall(owner, (), result),
        Err(failed) => Ended::fail_stop(failed.fail_stop(), Err("group disable")),
    }
}
