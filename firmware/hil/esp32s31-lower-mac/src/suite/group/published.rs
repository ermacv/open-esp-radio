//! Removal waits for the actual published AP GTK use to complete.
use super::*;
use oer_ieee80211_port::conformance::group_keys::TxOwners;
use oer_ieee80211_wire::{ccmp::CcmpPacketNumber, qos::WmmAccessCategory};
async fn exchange<'p, 'id: 'p>(
    phase: &mut Esp32s31Phase<'p, 'id, Device, TX>,
    resources: <Backend as PortBackend>::Resources<'id>,
    peer: Peer,
) -> Result<ProgressTrace<Events, Retained>, &'static str> {
    let (start, bank) = resources.access_point_association.into_parts();
    let (_, [slot, ..]) = bank.into_parts();
    let association = start.begin(Spp::NotNegotiated);
    let id = GroupKeyId::new(1).ok_or("KeyID")?;
    phase.access_point().install_group_key(
        &association,
        resources.access_point_group_permission,
        slot,
        id,
        Ccmp128Key::new(&[1; 16]),
    );
    let (key, permission) = loop {
        match phase.next_event().await {
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::AccessPoint { key, permission },
            )))) => break (key, permission),
            Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
            Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
            _ => return Err("AP GTK setup"),
        }
    };
    let admitted = Owners {
        slot: PortKeys::<AccessPoint>::group_slot_identity(&key),
        permission: permission.index(),
    };
    let [ticket, ..] = resources.tickets;
    let [mut buffer, ..] = resources.buffers;
    buffer.bytes_mut().fill(0);
    let published = TxOwners {
        ticket: ticket.index(),
        buffer: usize::from(buffer.index()),
    };
    let rate = phase.rate(PhyRate::Legacy(peer.rate)).ok_or("group rate")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x4208).ok_or("group FC")?,
        0,
        GroupAddress::new([0xff; 6]).ok_or("group receiver")?,
        peer.station,
        peer.address.get(),
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
    let source = <Family as TxBufferPool<BufferClass<TX>>>::prepare::<AccessPoint, Caps>(
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
    irq::hold_completion(true);
    phase.debug_hold_completion(true);
    oer_trace::start(port::trace::Publication::CHANNEL.mask());
    phase
        .access_point()
        .submit(
            Attempt {
                context: Context::Plain,
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
    barrier::progress().await;
    if TRACE
        .records()
        .filter(|row| row.decode::<port::trace::Publication>().is_some())
        .count()
        != 1
    {
        return Err("one AP publication required");
    }
    let admitted_at = EmbassyClock.now();
    phase.access_point().remove_group_key(permission, key);
    barrier::progress().await;
    let blocked_while_published = !TRACE
        .records()
        .any(|row| row.decode::<port::trace::TerminalPosted>().is_some());
    irq::hold_completion(false);
    phase.debug_hold_completion(false);
    let mut access = resources.terminal_access;
    let mut events = Events::new();
    while events.len() < 2 {
        let terminal = match phase.next_event().await {
            Ok(Ok(LowerMacEvent::Terminal(terminal))) => terminal,
            Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                phase.recycle(frame);
                continue;
            }
            Ok(Ok(LowerMacEvent::Tbtt(_))) => continue,
            _ => return Err("group removal terminal"),
        };
        let event = match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(tx)) => {
                let parts = tx.into_parts(&mut access);
                let (buffer, _) = parts.payload.into_parts();
                let AnyBuffer::Sent(sent) = buffer else {
                    return Err("published AP returned Fresh");
                };
                Completion::Transmit(TxOwners {
                    ticket: parts.ticket.index(),
                    buffer: usize::from(sent.index()),
                })
            }
            terminal => super::observed(terminal)?,
        };
        events.push(event).map_err(|_| "group capture capacity")?;
    }
    let completed_at = EmbassyClock.now();
    Ok(ProgressTrace {
        admitted,
        timing: Timing {
            admitted_at,
            completed_at,
        },
        events,
        retained: Retained::new(),
        poisoned: false,
        deadline_woke: false,
        published: Some(published),
        blocked_while_published,
    })
}
pub(super) async fn run<'p, 'id>(
    mut owner: Owner<'p, 'id, Backend, radio::Enabled>,
    resources: <Backend as PortBackend>::Resources<'id>,
    peer: Peer,
) -> Ended<'id, Backend, Result<Trace, &'static str>> {
    let mut result = exchange(&mut owner.phase(), resources, peer).await;
    irq::hold_completion(false);
    owner.phase().debug_hold_completion(false);
    let mut invalid = false;
    let disabled = owner
        .disable(&mut |terminal| {
            match (result.as_mut(), super::observed(terminal)) {
                (Ok(trace), Ok(event)) => {
                    if trace.events.push(event).is_err() {
                        invalid = true;
                    }
                }
                _ => invalid = true,
            }
            core::ops::ControlFlow::Continue(())
        })
        .await;
    if invalid {
        result = Err("extra group terminal");
    }
    let result = result.and_then(|raw| {
        group_trace(&raw, Backend::GROUP_COMMAND_BOUND.as_micros())
            .map(Trace::Group)
            .ok_or("group trace extent")
    });
    match disabled {
        Ok(owner) => Ended::uninstall(owner, (), result),
        Err(failed) => Ended::fail_stopped(failed.fail_stop(), Err("group disable")),
    }
}
