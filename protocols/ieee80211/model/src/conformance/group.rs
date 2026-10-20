//! Healthy GTK progress observed through actual terminals and disable drain.
use super::*;
use oer_ieee80211_port::conformance::group_keys::{
    Completion, Mutation, Owners, ProgressCase, ProgressTrace, Timing,
};
use oer_ieee80211_wire::{
    ccmp::{Ccmp128Key, GroupKeyId},
    security::Spp,
};
pub type GroupTrace = ProgressTrace<Vec<Completion>, Vec<Owners>>;
mod published;

/// Run a healthy command, preserving both its returned slot and permission.
pub async fn group(
    case: ProgressCase,
    channel: Channel,
    rate: LegacyRate,
    clock: Rc<dyn Clock>,
) -> Result<GroupTrace, &'static str> {
    if !matches!(
        case,
        ProgressCase::Disable { .. } | ProgressCase::PublishedRemoval
    ) {
        return Err("group publication fixture required");
    }
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("AP")?;
    let (backend, air) = ModelBackend::new(channel, [station, ap], clock);
    if case == ProgressCase::PublishedRemoval {
        return match oer_radio_port::run_port(backend, published::Exercise { air, rate }).await {
            PortEnd::Uninstalled { output, .. } => output,
            _ => Err("published GTK failed to uninstall"),
        };
    }
    match oer_radio_port::run_port(backend, Exercise { case, air }).await {
        PortEnd::Uninstalled { output, .. } => output,
        _ => Err("group fixture failed to uninstall"),
    }
}
struct Exercise {
    case: ProgressCase,
    air: Rc<ModelAir>,
}
fn observed(terminal: LowerMacTerminal<'_, ModelTypes>) -> Result<Completion, &'static str> {
    let (mutation, slot, permission) = match terminal {
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }) => {
            (Mutation::Install, key.slot.index(), permission.index())
        }
        LowerMacTerminal::GroupKeyReplaced { key, permission } => {
            (Mutation::Replace, key.slot.index(), permission.index())
        }
        LowerMacTerminal::GroupKeySelected { key, permission } => {
            (Mutation::Select, key.slot.index(), permission.index())
        }
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
async fn installed<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
) -> Result<
    (
        ModelGroupKey<'id, Station>,
        Slot<'id, ModelGroupPermission<Station>, ModelPool<8>>,
    ),
    &'static str,
> {
    match phase.next_event().await {
        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
            InstalledGroup::Station { key, permission },
        )))) => Ok((key, permission)),
        _ => Err("station GTK setup"),
    }
}
async fn admit<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    resources: ModelResources<'id>,
    air: &ModelAir,
    mutation: Mutation,
) -> Result<(Owners, Timing), &'static str> {
    let association =
        ModelKeys::<Station>::begin_association(resources.station_association, Spp::NotNegotiated);
    let [slot, ..] = resources.station_group;
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
            Some(k) => k.slot.index(),
            None => vacant.as_ref().ok_or("vacant GTK")?.index(),
        },
        permission: permission.index(),
    };
    let now = Instant::from_micros(air.monotonic_now().as_micros());
    air.hold_group_commands(true);
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
            id,
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
impl PortBody<ModelBackend> for Exercise {
    type Output = Result<GroupTrace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("group enable")),
        };
        let result = match self.case {
            ProgressCase::Disable { mutation, applied } => {
                let setup = admit(&mut owner.phase(), resources, &self.air, mutation).await;
                if applied {
                    self.air.hold_group_commands(false);
                    for _ in 0..4 {
                        super::configure::yield_once().await;
                    }
                }
                setup
            }
            _ => Err("healthy group disable case required"),
        };
        let mut events = Vec::new();
        let mut completed_at = Instant::from_micros(self.air.monotonic_now().as_micros());
        let mut invalid = false;
        let disabled = owner
            .disable(&mut |terminal| {
                match observed(terminal) {
                    Ok(event) => events.push(event),
                    Err(_) => invalid = true,
                }
                completed_at = Instant::from_micros(self.air.monotonic_now().as_micros());
                core::ops::ControlFlow::Continue(())
            })
            .await;
        let result = result.and_then(|(admitted, mut timing)| {
            if invalid {
                return Err("group terminal identity");
            }
            timing.completed_at = completed_at;
            Ok(ProgressTrace {
                admitted,
                timing,
                events,
                retained: Vec::new(),
                poisoned: self.air.failed(),
                deadline_woke: false,
                published: None,
                blocked_while_published: false,
            })
        });
        match disabled {
            Ok(owner) => Ended::uninstall(owner, (), result),
            Err(failed) => Ended::fail_stopped(failed.fail_stop(), Err("group disable")),
        }
    }
}
