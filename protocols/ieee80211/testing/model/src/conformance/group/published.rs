use super::*;
use oer_ieee80211_testing_conformance::group_keys::TxOwners;
use oer_ieee80211_wire::{ccmp::CcmpPacketNumber, qos::WmmAccessCategory};
pub(super) struct Exercise {
    pub air: Rc<ModelAir>,
    pub rate: LegacyRate,
}
async fn exchange<'p, 'id: 'p, Id: SlotIdentity + 'id>(
    phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
    resources: ModelResources<'id, ModelBackend<Id>>,
    body: &Exercise,
) -> Result<GroupTrace, &'static str> {
    let association = ModelKeys::<ModelBackend<Id>, AccessPoint>::begin_association(
        resources.access_point_association,
        Spp::NotNegotiated,
    );
    let [slot, ..] = resources.access_point_group;
    let id = GroupKeyId::new(1).ok_or("KeyID")?;
    phase.access_point().install_group_key(
        &association,
        resources.access_point_group_permission,
        slot,
        id,
        Ccmp128Key::new(&[1; 16]),
    );
    let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
        InstalledGroup::AccessPoint { key, permission },
    )))) = phase.next_event().await
    else {
        return Err("AP GTK setup");
    };
    let admitted = Owners {
        slot: key.slot.index(),
        permission: permission.index(),
    };
    let ticket = resources
        .tickets
        .into_iter()
        .next()
        .ok_or("published ticket")?;
    let buffer = resources
        .small
        .into_iter()
        .next()
        .ok_or("published buffer")?;
    let published = TxOwners {
        ticket: ticket.index(),
        buffer: usize::from(buffer.slot.index()),
    };
    let rate = phase.rate(PhyRate::Legacy(body.rate)).ok_or("group rate")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x4208).ok_or("group FC")?,
        0,
        GroupAddress::new([0xff; 6]).ok_or("group receiver")?,
        [2, 0, 0, 0, 0, 2],
        [2, 0, 0, 0, 0, 9],
        0,
        None,
        None,
        None,
    )
    .ok_or("group header")?;
    let frame = ClassFrame::new(TxFrame::Group {
        frame: SizedFrame::new(header, MpduLength::new(40).ok_or("group length")?)
            .ok_or("group frame")?,
        rate,
    })
    .map_err(|_| "group class")?;
    let source = <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
        AccessPoint,
        ModelCaps,
    >(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Ccmp(NumberedKey {
                key: AttemptKey::Group(&key),
                packet_number: CcmpPacketNumber::new(1).ok_or("group PN")?,
            }),
        }
        .check()
        .map_err(|_| "group layout")?,
    )
    .map_err(|_| "group prepare")?;
    body.air
        .hold_completion(WmmAccessCategory::BestEffort, true);
    phase
        .access_point()
        .submit(
            Attempt {
                context: (),
                ticket,
                source,
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).ok_or("group power")?,
                coex_operation: TxOperation::Data,
            }
            .admit(),
        )
        .map_err(|_| "group submit")?;
    for _ in 0..6 {
        super::super::configure::yield_once().await;
    }
    if body.air.sent().len() != 1 {
        return Err("one AP publication required");
    }
    let admitted_at = Instant::from_micros(body.air.monotonic_now().as_micros());
    phase.access_point().remove_group_key(permission, key);
    for _ in 0..6 {
        super::super::configure::yield_once().await;
    }
    let blocked_while_published = phase.held.events.is_empty() && body.air.sent().len() == 1;
    body.air
        .hold_completion(WmmAccessCategory::BestEffort, false);
    let mut access = resources.terminal_access;
    let mut events = Vec::new();
    for _ in 0..2 {
        let Ok(Ok(LowerMacEvent::Terminal(terminal))) = phase.next_event().await else {
            return Err("group removal terminal");
        };
        let event = match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(tx)) => {
                let parts = tx.into_parts(&mut access);
                let (buffer, _) = parts.payload.into_parts();
                let ModelAnyBuffer::SmallSent(sent) = buffer else {
                    return Err("published AP returned Fresh or another class");
                };
                Completion::Transmit(TxOwners {
                    ticket: parts.ticket.index(),
                    buffer: usize::from(sent.buffer.slot.index()),
                })
            }
            terminal => super::observed(terminal)?,
        };
        events.push(event);
    }
    let completed_at = Instant::from_micros(body.air.monotonic_now().as_micros());
    Ok(ProgressTrace {
        rollback: None,
        admitted,
        timing: Timing {
            admitted_at,
            completed_at,
        },
        events,
        retained: Vec::new(),
        poisoned: body.air.failed(),
        deadline_woke: false,
        published: Some(published),
        blocked_while_published,
    })
}
impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Exercise {
    type Output = Result<GroupTrace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, Self::Output>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => {
                return Ended::fail_stop(failed.fail_stop(), Err("group enable"));
            }
        };
        let mut result = exchange(&mut owner.phase(), resources, &self).await;
        self.air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        let mut invalid = false;
        let disabled = owner
            .disable(None, &mut |terminal| {
                match (result.as_mut(), super::observed(terminal)) {
                    (Ok(trace), Ok(event)) => trace.events.push(event),
                    _ => invalid = true,
                }
                core::ops::ControlFlow::Continue(())
            })
            .await;
        if invalid {
            result = Err("extra group terminal");
        }
        match disabled {
            Ok(owner) => Ended::uninstall(owner, (), result),
            Err(failed) => Ended::fail_stop(failed.fail_stop(), Err("group disable")),
        }
    }
}
