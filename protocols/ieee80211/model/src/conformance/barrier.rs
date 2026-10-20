use super::*;
use oer_ieee80211_port::conformance::{BarrierCase, BarrierEvent, BarrierTrace, HeldSnapshot};
use oer_ieee80211_wire::qos::WmmAccessCategory;
pub type Trace = BarrierTrace<Vec<BarrierEvent<ModelAck, ModelAirDenied>>, Vec<u8>, Vec<Vec<u8>>>;
pub async fn barrier(
    case: BarrierCase,
    channel: Channel,
    rate: LegacyRate,
    clock: Rc<dyn Clock>,
) -> Result<Trace, &'static str> {
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("model station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("model AP")?;
    let (backend, air) = ModelBackend::new(channel, [station, ap], clock);
    match oer_radio_port::run_port(backend, Body { case, rate, air }).await {
        PortEnd::Uninstalled { output, .. } => output,
        _ => Err("model barrier uninstall"),
    }
}
struct Body {
    case: BarrierCase,
    rate: LegacyRate,
    air: Rc<ModelAir>,
}
impl PortBody<ModelBackend> for Body {
    type Output = Result<Trace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("model enable")),
        };
        let result = exercise(&mut owner.phase(), resources, self).await;
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("model disable")),
        };
        Ended::uninstall(owner, (), result)
    }
}
async fn yield_once() {
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
fn snapshot(phase: &ModelPhase<'_, '_>) -> HeldSnapshot {
    HeldSnapshot {
        publications: phase.held.air.sent().len(),
        events: phase.held.events.items.borrow().len(),
    }
}
fn frame<'p, 'id: 'p>(
    phase: &ModelPhase<'p, 'id>,
    rate: LegacyRate,
) -> Result<TxFrame<'p, 'id, ModelCaps>, &'static str> {
    let peer = IndividualAddress::new([2, 0, 0, 0, 0, 9]).ok_or("model peer")?;
    let rate = phase
        .rate(PhyRate::Legacy(rate))
        .ok_or("model barrier rate")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x4108).ok_or("protected data control")?,
        0,
        peer,
        [2, 0, 0, 0, 0, 1],
        peer.get(),
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .ok_or("model protected header")?;
    Ok(TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).ok_or("model length")?)
            .ok_or("model frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(2).ok_or("retry budget")?,
            protection: Protection::None,
        }),
    })
}
async fn exercise<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    resources: ModelResources<'id>,
    body: Body,
) -> Result<Trace, &'static str> {
    if body.case == BarrierCase::GateTimeout {
        return Err("hardware barrier required");
    }
    let mut access = resources.terminal_access;
    let association = ModelKeys::<Station>::begin_association(
        resources.station_association,
        oer_ieee80211_wire::security::Spp::NotNegotiated,
    );
    let mut slots = resources.station_pairwise.into_iter();
    let mut key = Some(phase.station().install_pairwise_key(
        &association,
        slots.next().ok_or("model key slot")?,
        IndividualAddress::new([2, 0, 0, 0, 0, 9]).ok_or("model peer")?,
        Ccmp128Key::new(&[7; 16]),
    ));
    body.air.answer(if body.case == BarrierCase::GateAck {
        AirAnswer::Ack
    } else {
        AirAnswer::NoResponse
    });
    body.air
        .hold_completion(WmmAccessCategory::BestEffort, true);
    let mut tickets = resources.tickets.into_iter();
    let mut buffers = resources.small.into_iter();
    let mut identities = [0; 2];
    for (ordinal, identity) in identities.iter_mut().enumerate() {
        let ticket = tickets.next().ok_or("model ticket")?;
        *identity = ticket.index();
        let mut buffer = buffers.next().ok_or("model buffer")?;
        buffer.bytes_mut().fill(0);
        let source = TxSource::Fresh {
            buffer,
            frame: ClassFrame::new(frame(phase, body.rate)?).map_err(|_| "model class")?,
            body: None,
            protection: TxProtection::Ccmp(NumberedKey {
                key: key.as_ref().ok_or("model key")?,
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
        .map_err(|_| "model layout")?;
        let source = prepare_model(source).map_err(|_| "model preparation")?;
        phase
            .station()
            .submit(
                Attempt {
                    context: (),
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).ok_or("model power")?,
                    coex_operation: TxOperation::Data,
                }
                .admit(),
            )
            .map_err(|_| "model submit")?;
    }
    for _ in 0..6 {
        yield_once().await;
    }
    let before_request = snapshot(phase);
    let first_frame = body
        .air
        .sent()
        .first()
        .ok_or("model first publication")?
        .plaintext
        .clone();
    if body.case == BarrierCase::RemoveKey {
        phase
            .station()
            .remove_pairwise_key(association, key.take().ok_or("model removal key")?);
    } else {
        phase.close_gate(resources.gate);
    }
    for _ in 0..6 {
        yield_once().await;
    }
    let before_result = snapshot(phase);
    body.air
        .hold_completion(WmmAccessCategory::BestEffort, false);
    let mut events = Vec::new();
    let mut head = None;
    let mut closed = None;
    while events.len() < 3 {
        match phase
            .next_event()
            .await
            .map_err(|_| "model barrier poisoned")?
            .map_err(|_| "model event lost")?
        {
            LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                terminal,
            ))) => {
                let parts = terminal.into_parts(&mut access);
                let ticket_index = parts.ticket.index();
                let (buffer, _) = parts.payload.into_parts();
                let state = match buffer {
                    ModelAnyBuffer::SmallSent(sent) => {
                        if Some(&parts.ticket.index()) == identities.first() {
                            head = Some((parts.ticket, sent));
                        }
                        MpduState::Sent
                    }
                    ModelAnyBuffer::LargeSent(_) => MpduState::Sent,
                    _ => MpduState::Fresh,
                };
                events.push(BarrierEvent::Tx {
                    ticket: ticket_index,
                    state,
                    outcome: parts.outcome,
                });
            }
            LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)) => {
                closed = Some(gate);
                events.push(BarrierEvent::GateClosed);
            }
            LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)) => {
                events.push(BarrierEvent::KeyRemoved)
            }
            _ => return Err("unexpected model barrier event"),
        }
    }
    let publications_at_barrier = body.air.sent().len();
    let mut retried_frames = Vec::new();
    if body.case == BarrierCase::GateRetry {
        let _open = phase.open_gate(closed.ok_or("model closed gate")?);
        let (ticket, mpdu) = head.take().ok_or("model Sent head")?;
        let TxFrame::Individual { plan, .. } = frame(phase, body.rate)? else {
            return Err("model retry frame");
        };
        let source = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(
            CheckedTxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        )
        .map_err(|_| "model retry preparation")?;
        phase
            .station()
            .submit(
                Attempt {
                    context: (),
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).ok_or("model power")?,
                    coex_operation: TxOperation::Data,
                }
                .admit(),
            )
            .map_err(|_| "model retry submit")?;
        loop {
            match phase
                .next_event()
                .await
                .map_err(|_| "model retry poisoned")?
                .map_err(|_| "model retry event lost")?
            {
                LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(_)) => break,
                LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
                _ => return Err("unexpected model retry event"),
            }
        }
        retried_frames = body
            .air
            .sent()
            .into_iter()
            .skip(1)
            .map(|frame| frame.plaintext)
            .collect();
    }
    Ok(BarrierTrace {
        tickets: identities,
        before_request,
        before_result,
        just_before_timeout: None,
        publications_at_barrier,
        events,
        first_frame,
        retried_frames,
    })
}
