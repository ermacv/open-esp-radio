//! Real publications with their actual completion facts held by the image.
use super::*;
use oer_hil_protocol::lower_mac::{Trace, barrier_trace};
use oer_ieee80211_port::conformance::{BarrierCase, BarrierEvent, BarrierTrace, HeldSnapshot};
use oer_ieee80211_wire::{ccmp::Ccmp128Key, qos::WmmAccessCategory};
type Frame = heapless::Vec<u8, 64>;
type Frames = heapless::Vec<Frame, 4>;
type Events = heapless::Vec<BarrierEvent<Ack, Infallible>, 8>;

pub(super) async fn run<'p, 'id, Id: radio::SlotIdentity + 'id>(
    mut owner: Owner<'p, 'id, Id, Backend<Id>, radio::Enabled>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: BarrierCase,
    output: &mut TraceScratch,
) -> Ended<Id, Backend<Id>, Result<Trace, &'static str>> {
    assert!(output.0.is_none(), "the previous HIL trace was consumed");
    let output = PendingTrace(output);
    let error = match run_case(pin!(exercise(&mut owner.phase(), resources, peer, case))).await {
        Ok(trace) => {
            output.0.0 = Some(trace);
            None
        }
        Err(error) => Some(error),
    };
    irq::hold_completion(false);
    owner.phase().debug_hold_completion(false);
    let owner = match owner
        .disable(&mut |_| core::ops::ControlFlow::Continue(()))
        .await
    {
        Ok(owner) => owner,
        Err(failed) => {
            return Ended::fail_stop(failed.fail_stop(), Err("barrier disable"));
        }
    };
    let result = match error {
        Some(error) => Err(error),
        None => Ok(output.0.0.take().expect("one successful HIL trace")),
    };
    Ended::uninstall(owner, (), result)
}
pub(super) async fn progress() {
    for _ in 0..6 {
        let mut yielded = false;
        core::future::poll_fn(|cx| {
            if yielded {
                core::task::Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        })
        .await;
    }
}
fn publications() -> usize {
    TRACE
        .records()
        .filter(|r| r.decode::<port::trace::Publication>().is_some())
        .count()
}
fn snapshot() -> HeldSnapshot {
    HeldSnapshot {
        publications: publications(),
        events: TRACE
            .records()
            .filter(|r| r.decode::<port::trace::TerminalPosted>().is_some())
            .count(),
    }
}
fn frames() -> Result<Frames, &'static str> {
    let mut frames = Frames::new();
    for snapshot in TRACE
        .snapshots()
        .filter(|snapshot| snapshot.point == port::trace::ENCODED_MPDU.raw())
    {
        if snapshot.truncated {
            return Err("truncated encoded MPDU");
        }
        let mut words = [0; 33];
        let count = TRACE
            .read_snapshot(&snapshot, 0, &mut words)
            .ok_or("stale encoded MPDU")?;
        let len = *words.first().ok_or("encoded length")? as usize;
        if len > 64 {
            return Err("encoded frame capacity");
        }
        let mut frame = Frame::new();
        for byte in words
            .get(1..count)
            .ok_or("encoded snapshot words")?
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .take(len)
        {
            frame.push(byte).map_err(|_| "encoded frame overflow")?;
        }
        if frame.len() != len {
            return Err("short encoded MPDU");
        }
        frames.push(frame).map_err(|_| "too many encoded frames")?;
    }
    Ok(frames)
}
fn frame<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &Esp32s31Phase<'p, 'id, Id, Device, TX>,
    peer: Peer,
    receiver: IndividualAddress,
) -> Result<TxFrame<'p, 'id, Caps>, &'static str> {
    let rate = phase
        .rate(PhyRate::Legacy(peer.rate))
        .ok_or("barrier rate")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x4108).ok_or("protected data control")?,
        0,
        receiver,
        peer.station,
        peer.address.get(),
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .ok_or("protected header")?;
    Ok(TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).ok_or("length")?)
            .ok_or("protected frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(2).ok_or("retry budget")?,
            protection: Protection::None,
        }),
    })
}
async fn exercise<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: BarrierCase,
) -> Result<Trace, &'static str> {
    if case == BarrierCase::GateTimeout {
        return Err("physical barrier required");
    }
    let mut access = resources.terminal_access;
    let (start, slot, _vacant) =
        PortKeys::<Id, Station>::split_vacant::<1>(resources.station_association);
    let association = PortKeys::<Id, Station>::begin_association(
        start,
        oer_ieee80211_wire::security::Spp::NotNegotiated,
    );
    let receiver = if case == BarrierCase::GateAck {
        peer.address
    } else {
        IndividualAddress::new([2, 0, 0, 0, 0, 254]).ok_or("absent peer")?
    };
    let mut key = Some(phase.station().install_pairwise_key(
        &association,
        slot,
        receiver,
        Ccmp128Key::new(&[7; 16]),
    ));
    irq::hold_completion(true);
    phase.debug_hold_completion(true);
    oer_trace::start(port::trace::Publication::CHANNEL.mask());
    let mut tickets = resources.tickets.into_iter();
    let mut buffers = resources.buffers.into_iter();
    let mut identities = [0; 2];
    for (ordinal, identity) in identities.iter_mut().enumerate() {
        let ticket = tickets.next().ok_or("barrier ticket")?;
        *identity = ticket.index();
        let mut buffer = buffers.next().ok_or("barrier buffer")?;
        buffer.bytes_mut().fill(0);
        let source = TxSource::Fresh {
            buffer,
            frame: ClassFrame::new(frame(phase, peer, receiver)?).map_err(|_| "barrier class")?,
            body: None,
            protection: TxProtection::Ccmp(NumberedKey {
                key: key.as_ref().ok_or("barrier key")?,
                packet_number: oer_ieee80211_wire::ccmp::CcmpPacketNumber::new(
                    u64::try_from(ordinal)
                        .map_err(|_| "PN ordinal")?
                        .checked_add(1)
                        .ok_or("PN bound")?,
                )
                .ok_or("PN")?,
            }),
        }
        .check()
        .map_err(|_| "protected layout")?;
        let source =
            <Family<Id> as TxBufferPool<BufferClass<TX>>>::prepare::<Station, Caps>(source)
                .map_err(|_| "protected preparation")?;
        phase
            .station()
            .submit(
                Attempt {
                    context: Context::Plain,
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).ok_or("power")?,
                    coex_operation: TxOperation::Data,
                }
                .admit(),
            )
            .map_err(|_| "barrier submit")?;
    }
    progress().await;
    let before_request = snapshot();
    let first_frame = frames()?
        .into_iter()
        .next()
        .ok_or("first real publication frame")?;
    if case == BarrierCase::RemoveKey {
        phase
            .station()
            .remove_pairwise_key(association, key.take().ok_or("removal key")?);
    } else {
        phase.close_gate(resources.gate);
    }
    progress().await;
    let before_result = snapshot();
    irq::hold_completion(false);
    phase.debug_hold_completion(false);
    let mut events = Events::new();
    let mut head = None;
    let mut closed = None;
    while events.len() < 3 {
        let event = phase
            .next_event()
            .await
            .map_err(|_| "barrier poisoned")?
            .map_err(|_| "barrier lost event")?;
        let event = match event {
            LowerMacEvent::Received { frame, .. } => {
                phase.recycle(frame);
                continue;
            }
            LowerMacEvent::Tbtt(_) => continue,
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                terminal,
            ))) => {
                let parts = terminal.into_parts(&mut access);
                let ticket = parts.ticket.index();
                let (buffer, _) = parts.payload.into_parts();
                let state = match buffer {
                    AnyBuffer::Sent(sent) => {
                        if Some(&ticket) == identities.first() {
                            head = Some((parts.ticket, sent));
                        }
                        MpduState::Sent
                    }
                    AnyBuffer::Fresh(_) => MpduState::Fresh,
                };
                BarrierEvent::Tx {
                    ticket,
                    state,
                    outcome: parts.outcome,
                }
            }
            LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)) => {
                closed = Some(gate);
                BarrierEvent::GateClosed
            }
            LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)) => BarrierEvent::KeyRemoved,
            _ => return Err("unexpected barrier event"),
        };
        events.push(event).map_err(|_| "too many barrier events")?;
    }
    let publications_at_barrier = publications();
    let mut retried_frames = Frames::new();
    let mut retry_terminal = None;
    if case == BarrierCase::GateRetry {
        let _open = phase.open_gate(closed.ok_or("closed gate")?);
        let (ticket, mpdu) = head.take().ok_or("Sent barrier head")?;
        let TxFrame::Individual { plan, .. } = frame(phase, peer, receiver)? else {
            return Err("retry frame");
        };
        let source = <Family<Id> as TxBufferPool<BufferClass<TX>>>::prepare::<Station, Caps>(
            CheckedTxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        )
        .map_err(|_| "retry preparation")?;
        phase
            .station()
            .submit(
                Attempt {
                    context: Context::Plain,
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).ok_or("power")?,
                    coex_operation: TxOperation::Data,
                }
                .admit(),
            )
            .map_err(|_| "retry submit")?;
        loop {
            match phase
                .next_event()
                .await
                .map_err(|_| "retry poisoned")?
                .map_err(|_| "retry lost event")?
            {
                LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                    completed,
                ))) => {
                    let parts = completed.into_parts(&mut access);
                    let (buffer, _) = parts.payload.into_parts();
                    let state = match buffer {
                        AnyBuffer::Sent(_) => MpduState::Sent,
                        AnyBuffer::Fresh(_) => MpduState::Fresh,
                    };
                    retry_terminal = Some(BarrierEvent::Tx {
                        ticket: parts.ticket.index(),
                        state,
                        outcome: parts.outcome,
                    });
                    break;
                }
                LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
                LowerMacEvent::Tbtt(_) => {}
                _ => return Err("unexpected retry event"),
            }
        }
        for frame in frames()?.into_iter().skip(1) {
            retried_frames
                .push(frame)
                .map_err(|_| "too many retry frames")?;
        }
    }
    let trace = BarrierTrace {
        tickets: identities,
        before_request,
        before_result,
        just_before_timeout: None,
        publications_at_barrier,
        events,
        first_frame,
        retried_frames,
        retry_terminal,
    };
    barrier_trace(&trace)
        .map(Trace::Barrier)
        .ok_or("barrier trace extent")
}
