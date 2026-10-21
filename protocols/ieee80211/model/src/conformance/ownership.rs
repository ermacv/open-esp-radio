//! Ownership observations from ordinary ModelBackend operations.
use super::*;
use conformance::allocations::{Allocation, Ids};
use oer_ieee80211_port::conformance::{self, Class, End, Operation, Resources, Scenario};
use oer_ieee80211_wire::qos::WmmAccessCategory;
#[derive(Debug)]
struct Body {
    memory: ModelBody,
    allocation: Allocation<Rc<Ids<4>>, 4>,
}
impl TxBody for Body {
    fn bytes(&self) -> &[u8] {
        self.memory.bytes()
    }
}
#[derive(Default)]
struct Capture {
    rows: Vec<conformance::Returned>,
    pointers: Vec<conformance::ReturnEvidence>,
    gate_closed_at: Option<usize>,
}
impl Capture {
    fn len(&self) -> usize {
        self.rows.len()
    }
}
/// Actual ownership, terminal and publication/barrier observations.
pub type OwnershipTrace = conformance::Trace<
    Vec<conformance::Returned>,
    Vec<conformance::ReturnEvidence>,
    Vec<conformance::PublicationEvent>,
>;

struct Context {
    operation: Operation,
    resources: Resources,
    pointer: usize,
    drops: Rc<Cell<usize>>,
}
impl Drop for Context {
    fn drop(&mut self) {
        self.drops.set(self.drops.get().saturating_add(1));
    }
}
struct Contexts;
impl ModelContext for Contexts {
    type Body = Body;
    type OwnedContext<Id: SlotIdentity> = Context;
    type Context<'p, Id: SlotIdentity + 'p> = Context;
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}
type Backend<Id> = ModelBackend<Id, ModelCaps, Contexts>;

/// Exercise the same four owned operations as the physical fixture.
pub async fn ownership(
    scenario: Scenario,
    channel: Channel,
    rate: LegacyRate,
    clock: Rc<dyn Clock>,
) -> Result<OwnershipTrace, &'static str> {
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("model station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("model AP")?;
    let (backend, air) = Fixture::<ModelCaps, Contexts>::new(channel, [station, ap], clock);
    if scenario == Scenario::CloseGate {
        // Match the physical fixture's absent receiver on every publication.
        // Retry exhaustion is determined by the unchanged operation plan.
        air.absent_peer.set(true);
    }
    match run_model(
        backend,
        Operations {
            air,
            scenario,
            rate,
        },
    )
    .await
    {
        Report::Joined(output) => output,
        _ => Err("model ownership failed to uninstall"),
    }
}
fn observe<'id, Id: SlotIdentity + 'id>(
    context: Context,
    ticket: Slot<'id, ModelTickets, ModelPool<Id, 1>>,
    buffer: ModelAnyBuffer<'id, Backend<Id>>,
    body: Option<Body>,
    (end, evidence): (End, conformance::ReturnEvidence),
    seen: &mut Capture,
) -> Result<(), &'static str> {
    let (class, index, body) = match buffer {
        ModelAnyBuffer::Small(buffer) => (Class::Small, buffer.slot.index(), body),
        ModelAnyBuffer::Large(buffer) => (Class::Large, buffer.slot.index(), body),
        ModelAnyBuffer::SmallSent(sent) => {
            let (buffer, body) = <ModelTypes<Backend<Id>> as TxBufferPool<ModelSmall>>::renew(sent);
            (Class::Small, buffer.slot.index(), body)
        }
        ModelAnyBuffer::LargeSent(sent) => {
            let (buffer, body) = <ModelTypes<Backend<Id>> as TxBufferPool<ModelLarge>>::renew(sent);
            (Class::Large, buffer.slot.index(), body)
        }
    };
    let body = body.ok_or("model lost external body")?;
    if !body
        .bytes()
        .iter()
        .all(|byte| *byte == context.operation.id)
    {
        return Err("model changed external body");
    }
    seen.pointers
        .push(evidence.with_pointer(context.pointer == body.bytes().as_ptr() as usize));
    seen.rows.push(conformance::Returned {
        operation: context.operation,
        expected: context.resources,
        actual: Resources {
            ticket: ticket.index(),
            buffer: index,
            class,
            body: usize::from(body.allocation.id()),
        },
        end,
    });
    Ok(())
}
fn returned<'id, Id: SlotIdentity + 'id>(
    terminal: LowerMacTerminal<'id, ModelTypes<Backend<Id>>>,
    access: &mut oer_ieee80211_port::TerminalAccess<'id, ModelTickets, ModelPool<Id, 1>>,
    seen: &mut Capture,
    air: &ModelAir,
) -> Result<(), &'static str> {
    match terminal {
        LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
            let parts = terminal.into_parts(access);
            let evidence = conformance::ReturnEvidence::terminal(false, &parts.outcome)
                .ok_or("ownership terminal evidence extent")?;
            let end = match parts.outcome {
                TxOutcome::Ended(_) => End::Ended,
                TxOutcome::Withdrawn(_) => End::Withdrawn,
                _ => return Err("unexpected model ownership terminal"),
            };
            let (buffer, body) = parts.payload.into_parts();
            observe(
                parts.context,
                parts.ticket,
                buffer,
                body,
                (end, evidence),
                seen,
            )
        }
        LowerMacTerminal::GateClosed(_) => {
            if seen.gate_closed_at.replace(air.sent().len()).is_some() {
                return Err("duplicate model gate confirmation");
            }
            Ok(())
        }
        _ => Err("unexpected model ownership event"),
    }
}
fn frame<'p, 'id: 'p, Id: SlotIdentity + 'id>(
    phase: &ModelPhase<'p, 'id, ModelCaps, Backend<Id>>,
    rate: LegacyRate,
    scenario: Scenario,
) -> Result<TxFrame<'p, 'id, ModelCaps>, &'static str> {
    let rate = phase
        .rate(PhyRate::Legacy(rate))
        .ok_or("model ownership rate")?;
    let peer = IndividualAddress::new(if scenario == Scenario::CloseGate {
        [2, 0, 0, 0, 0, 254]
    } else {
        [2, 0, 0, 0, 0, 9]
    })
    .ok_or("model peer")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x0108).ok_or("model data control")?,
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
    .ok_or("model data header")?;
    Ok(TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).ok_or("model length")?)
            .ok_or("model sized frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(2).ok_or("retry budget")?,
            protection: Protection::None,
        }),
    })
}
struct Operations {
    air: Rc<ModelAir>,
    scenario: Scenario,
    rate: LegacyRate,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Operations {
    type Output = Result<OwnershipTrace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, Backend<Id>, Disabled>,
        resources: ModelResources<'id, Backend<Id>>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => {
                return Ended::fail_stop(failed.fail_stop(), Err("model enable"));
            }
        };
        let drops = Rc::new(Cell::new(0));
        let mut seen = Capture::default();
        let ids = Rc::new(Ids::<4>::new());
        let mut access = resources.terminal_access;
        let result = async {
            let mut tickets = resources.tickets.into_iter();
            let mut small = resources.small.into_iter();
            let mut large = resources.large.into_iter();
            let mut phase = owner.phase();
            self.air.hold(WmmAccessCategory::BestEffort, true);
            for operation in conformance::OPERATIONS {
                let ticket = tickets.next().ok_or("model ticket")?;
                let body = Body {
                    memory: ModelBody(alloc::vec![operation.id; if self.scenario == Scenario::Refuse {17} else {4}]),
                    allocation: Allocation::new(ids.clone()).ok_or("model body allocation slots exhausted")?,
                };
                let pointer = body.bytes().as_ptr() as usize;
                let body_id = body.allocation.id();
                macro_rules! source {
                    ($buffer:expr, $class:ty, $variant:ident) => {{
                        let mut buffer = $buffer;
                        buffer.bytes_mut().fill(0);
                        let index = buffer.slot.index();
                        let source = TxSource::Fresh {
                            buffer,
                            frame: ClassFrame::new(frame(&phase, self.rate, self.scenario)?)
                                .map_err(|_| "model class")?,
                            body: Some(body),
                            protection: TxProtection::Open,
                        }
                        .check();
                        let source = match source {
                            Ok(source) => {
                                Ok(<ModelTypes<Backend<Id>> as TxBufferPool<$class>>::prepare::<
                                    Station,
                                    ModelCaps,
                                >(source)
                                .map_err(|_| "model preparation")?)
                            }
                            Err(refused) => match refused.operation {
                                TxSource::Fresh { buffer, body, .. } => {
                                    Err((ModelAnyBuffer::$variant(buffer), body))
                                }
                                _ => return Err("model fresh refusal"),
                            },
                        };
                        (index, source)
                    }};
                }
                let (index, source) = match operation.class {
                    Class::Small => {
                        source!(small.next().ok_or("model small buffer")?, ModelSmall, Small)
                    }
                    Class::Large => {
                        source!(large.next().ok_or("model large buffer")?, ModelLarge, Large)
                    }
                };
                let context = Context {
                    operation,
                    resources: Resources {
                        ticket: ticket.index(),
                        buffer: index,
                        class: operation.class,
                        body: usize::from(body_id),
                    },
                    pointer,
                    drops: drops.clone(),
                };
                match source {
                    Ok(source) => phase
                        .station()
                        .submit(
                            Attempt {
                                context,
                                ticket,
                                source,
                                access_category: operation.category,
                                protection: Protection::None,
                                power: SupportedPower::new(TxPower::Calibrated)
                                    .ok_or("model power")?,
                                coex_operation: TxOperation::Data,
                            }
                            .admit(),
                        )
                        .map_err(|_| "model ownership submit")?,
                    Err((buffer, body)) => {
                        observe(context, ticket, buffer, body, (End::Refused,conformance::ReturnEvidence::refused(false)), &mut seen)?
                    }
                }
            }
            if self.scenario != Scenario::Refuse {
                while seen.len() < 2 {
                    match phase
                        .next_event()
                        .await
                        .map_err(|_| "model ownership poisoned")?
                        .map_err(|_| "model ownership lost event")?
                    {
                        LowerMacEvent::Terminal(terminal) => {
                            returned(terminal, &mut access, &mut seen, &self.air)?
                        }
                        LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
                        _ => {}
                    }
                }
                match self.scenario {
                    Scenario::Complete => self.air.hold(WmmAccessCategory::BestEffort, false),
                    Scenario::CloseGate => phase.close_gate(resources.gate),
                    Scenario::Disable => {}
                    _ => return Err("physical ownership scenario required"),
                }
                if matches!(self.scenario, Scenario::Complete | Scenario::CloseGate) {
                    while seen.len() < 4 {
                        match phase
                            .next_event()
                            .await
                            .map_err(|_| "model ownership poisoned")?
                            .map_err(|_| "model ownership lost event")?
                        {
                            LowerMacEvent::Terminal(terminal) => {
                                returned(terminal, &mut access, &mut seen, &self.air)?
                            }
                            LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
                            _ => {}
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        let mut error = result.err();
        let owner = match owner
            .disable(&mut |terminal| {
                if let Err(why) = returned(terminal, &mut access, &mut seen, &self.air) {
                    error = Some(why);
                }
                core::ops::ControlFlow::Continue(())
            })
            .await
        {
            Ok(owner) => owner,
            Err(failed) => {
                return Ended::fail_stop(failed.fail_stop(), Err("model disable"));
            }
        };
        let output = match error {
            Some(why) => Err(why),
            None => {
                let mut publication_events: Vec<_> = self
                    .air
                    .sent()
                    .iter()
                    .map(|frame| conformance::PublicationEvent::Tx(frame.ticket))
                    .collect();
                if let Some(index) = seen.gate_closed_at {
                    publication_events.insert(index, conformance::PublicationEvent::GateClosed);
                }
                Ok(OwnershipTrace {
                    returned: seen.rows,
                    contexts_dropped: drops.get(),
                    before_teardown_dropped: drops.get(),
                    publications: self.air.sent().len(),
                    terminal_evidence: seen.pointers,
                    publication_events,
                })
            }
        };
        Ended::uninstall(owner, (), output)
    }
}

plain_context!(Context);
