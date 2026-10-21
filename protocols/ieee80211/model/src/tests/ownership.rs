use super::*;
use oer_ieee80211_port::conformance::{
    self, Class, End, Harness, Operation, Resources, Scenario, Trace,
};

// A fixture-local installation brand; this harness never creates a core epoch.
struct InstallBrand<'id>(PhantomData<fn(&'id ()) -> &'id ()>);

// Deliberately neither Copy nor Clone, and branded by the installation.
struct Context<'id> {
    epoch: Option<InstallBrand<'id>>,
    operation: Operation,
    resources: Resources,
    dropped: Rc<Cell<usize>>,
}
struct Contexts;
impl ModelContext for Contexts {
    type StationConfigureContext<'id> = ();
    type AccessPointConfigureContext<'id> = ();
    type Body = ModelBody;
    type Context<'id> = Context<'id>;
}
impl Drop for Context<'_> {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
type Backend = ModelBackend<ModelCaps, Contexts>;

struct Operations {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    before_teardown: Rc<Cell<usize>>,
    seen: Rc<RefCell<Vec<conformance::Returned>>>,
    scenario: Scenario,
    evidence: Rc<RefCell<Vec<conformance::ReturnEvidence>>>,
    gate_closed_at: Rc<Cell<Option<usize>>>,
}

fn observe<'id>(
    context: Context<'_>,
    ticket: Slot<'id, ModelTickets, ModelPool<'id, 1>>,
    buffer: ModelAnyBuffer<'_>,
    body: Option<ModelBody>,
    end: End,
    seen: &RefCell<Vec<conformance::Returned>>,
) {
    assert!(context.epoch.is_none());
    let (class, index, body) = match buffer {
        ModelAnyBuffer::Small(buffer) => (Class::Small, buffer.slot.index(), body),
        ModelAnyBuffer::Large(buffer) => (Class::Large, buffer.slot.index(), body),
        ModelAnyBuffer::SmallSent(mpdu) => {
            assert!(body.is_none());
            let (buffer, body) = <ModelTypes<Contexts> as TxBufferPool<ModelSmall>>::renew(mpdu);
            (Class::Small, buffer.slot.index(), body)
        }
        ModelAnyBuffer::LargeSent(mpdu) => {
            assert!(body.is_none());
            let (buffer, body) = <ModelTypes<Contexts> as TxBufferPool<ModelLarge>>::renew(mpdu);
            (Class::Large, buffer.slot.index(), body)
        }
    };
    let body = body.unwrap();
    assert!(body.0.iter().all(|byte| *byte == context.operation.id));
    seen.borrow_mut().push(conformance::Returned {
        operation: context.operation,
        expected: context.resources,
        actual: Resources {
            ticket: ticket.index(),
            buffer: index,
            class,
            body: body.0.as_ptr() as usize,
        },
        end,
    });
}

fn returned<'id>(
    event: LowerMacEvent<'id, ModelTypes<Contexts>>,
    seen: &RefCell<Vec<conformance::Returned>>,
    terminal_access: &mut oer_ieee80211_port::TerminalAccess<'id, ModelTickets, ModelPool<'id, 1>>,
    evidence: &RefCell<Vec<conformance::ReturnEvidence>>,
    gate_closed_at: &Cell<Option<usize>>,
    air: &ModelAir,
) {
    match event {
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal))) => {
            let oer_ieee80211_port::TerminalParts {
                context,
                ticket,
                outcome: completion,
                payload,
                ..
            } = terminal.into_parts(terminal_access);
            let (buffer, body) = payload.into_parts();
            evidence
                .borrow_mut()
                .push(conformance::ReturnEvidence::terminal(true, &completion).unwrap());

            let end = match completion {
                TxOutcome::Ended(SingleEnd::Delivered {
                    air:
                        Air {
                            transmissions: core::num::NonZeroU16::MIN,
                            ..
                        },
                    ..
                }) => End::Ended,
                TxOutcome::Withdrawn(_) => End::Withdrawn,
                other => panic!("unexpected outcome {other:?}"),
            };
            observe(context, ticket, buffer, body, end, seen);
        }
        LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(_)) => {
            assert!(gate_closed_at.replace(Some(air.sent().len())).is_none());
        }
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(_))) => {
            panic!("single submission must return Single")
        }
        _ => {}
    }
}

impl PortBody<Backend> for Operations {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, Backend, ()> {
        let mut terminal_access = resources.terminal_access;
        let Ok(mut owner) = owner.enable().await else {
            panic!("model enables")
        };
        let ModelResources {
            mut tickets,
            mut small,
            mut large,
            gate,
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            self.air.hold(WmmAccessCategory::BestEffort, true);
            for operation in conformance::OPERATIONS {
                let ticket = tickets.remove(0);
                let body = ModelBody(
                    alloc::vec![operation.id; if self.scenario == Scenario::Refuse { 17 } else { 4 }],
                );
                let (index, source) = match operation.class {
                    Class::Small => {
                        let buffer = small.remove(0);
                        let index = buffer.slot.index();
                        let source = TxSource::Fresh {
                            buffer,
                            frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                            body: Some(body),
                            protection: TxProtection::Open,
                        }
                        .check()
                        .map(|source| {
                            <ModelTypes<Contexts> as TxBufferPool<ModelSmall>>::prepare::<
                                Station,
                                ModelCaps,
                            >(source)
                            .ok()
                            .unwrap()
                        })
                        .map_err(|refused| match refused.operation {
                            TxSource::Fresh { buffer, body, .. } => {
                                (ModelAnyBuffer::Small(buffer), body)
                            }
                            TxSource::Retransmission { .. } => unreachable!(),
                        });
                        (index, source)
                    }
                    Class::Large => {
                        let buffer = large.remove(0);
                        let index = buffer.slot.index();
                        let source = TxSource::Fresh {
                            buffer,
                            frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                            body: Some(body),
                            protection: TxProtection::Open,
                        }
                        .check()
                        .map(|source| {
                            <ModelTypes<Contexts> as TxBufferPool<ModelLarge>>::prepare::<
                                Station,
                                ModelCaps,
                            >(source)
                            .ok()
                            .unwrap()
                        })
                        .map_err(|refused| match refused.operation {
                            TxSource::Fresh { buffer, body, .. } => {
                                (ModelAnyBuffer::Large(buffer), body)
                            }
                            TxSource::Retransmission { .. } => unreachable!(),
                        });
                        (index, source)
                    }
                };
                // Identity comes from the actual prepared/refused owner, before transfer.
                let body_pointer = match &source {
                    Ok(source) => source.mpdu.body.as_ref().unwrap().0.as_ptr(),
                    Err((_, body)) => body.as_ref().unwrap().0.as_ptr(),
                };
                let context = Context {
                    epoch: None,
                    operation,
                    resources: Resources {
                        ticket: ticket.index(),
                        buffer: index,
                        class: operation.class,
                        body: body_pointer as usize,
                    },
                    dropped: self.dropped.clone(),
                };
                match source {
                    Ok(source) => {
                        assert_ne!(self.scenario, Scenario::Refuse);
                        phase
                            .station()
                            .submit(
                                (oer_ieee80211_port::Attempt {
                                    context,
                                    ticket,
                                    source,
                                    access_category: operation.category,
                                    protection: Protection::None,
                                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                    coex_operation: oer_ieee80211_port::TxOperation::Data,
                                })
                                .admit(),
                            )
                            .unwrap();
                    }
                    Err((buffer, body)) => {
                        assert_eq!(self.scenario, Scenario::Refuse);
                        self.evidence
                            .borrow_mut()
                            .push(conformance::ReturnEvidence::refused(true));
                        observe(context, ticket, buffer, body, End::Refused, &self.seen);
                    }
                }
            }
            if self.scenario != Scenario::Refuse {
                while self.seen.borrow().len() < 2 {
                    let Ok(Ok(event)) = phase.next_event().await else {
                        panic!("unheld AC completes")
                    };
                    returned(
                        event,
                        &self.seen,
                        &mut terminal_access,
                        &self.evidence,
                        &self.gate_closed_at,
                        &self.air,
                    );
                }
                assert!(
                    self.seen
                        .borrow()
                        .iter()
                        .all(|observation| observation.operation.category
                            != WmmAccessCategory::BestEffort)
                );
                match self.scenario {
                    Scenario::Complete => self.air.hold(WmmAccessCategory::BestEffort, false),
                    Scenario::CloseGate => phase.close_gate(gate),
                    Scenario::Disable => {}
                    Scenario::Poison => self.air.poison(),
                    Scenario::Refuse => unreachable!(),
                }
                if matches!(self.scenario, Scenario::Complete | Scenario::CloseGate) {
                    while self.seen.borrow().len() < 4 {
                        let Ok(Ok(event)) = phase.next_event().await else {
                            panic!("terminal")
                        };
                        returned(
                            event,
                            &self.seen,
                            &mut terminal_access,
                            &self.evidence,
                            &self.gate_closed_at,
                            &self.air,
                        );
                    }
                }
            }
        }
        let mut sink = |event| {
            returned(
                LowerMacEvent::Terminal(event),
                &self.seen,
                &mut terminal_access,
                &self.evidence,
                &self.gate_closed_at,
                &self.air,
            );
            core::ops::ControlFlow::Continue(())
        };
        match owner.disable(&mut sink).await {
            Ok(owner) => {
                self.before_teardown.set(self.dropped.get());
                Ended::uninstall(owner, (), ())
            }
            Err(poisoned) => {
                self.before_teardown.set(self.dropped.get());
                Ended::fail_stop(poisoned.fail_stop(), ())
            }
        }
    }
}

struct ModelHarness;
impl Harness for ModelHarness {
    type Returns = Vec<conformance::Returned>;
    type Evidence = Vec<conformance::ReturnEvidence>;
    type Publications = Vec<conformance::PublicationEvent>;
    async fn exercise(
        &mut self,
        scenario: Scenario,
    ) -> Trace<Self::Returns, Self::Evidence, Self::Publications> {
        let (backend, air) = Backend::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        );
        let dropped = Rc::new(Cell::new(0));
        let before_teardown = Rc::new(Cell::new(usize::MAX));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let evidence = Rc::new(RefCell::new(Vec::new()));
        let gate_closed_at = Rc::new(Cell::new(None));
        let _port_end = run_port(
            backend,
            Operations {
                air: air.clone(),
                dropped: dropped.clone(),
                before_teardown: before_teardown.clone(),
                seen: seen.clone(),
                scenario,
                evidence: evidence.clone(),
                gate_closed_at: gate_closed_at.clone(),
            },
            oer_radio_port::LocalExecution,
        )
        .await;
        let mut publication_events: Vec<_> = air
            .sent()
            .iter()
            .map(|frame| conformance::PublicationEvent::Tx(frame.ticket))
            .collect();
        if let Some(index) = gate_closed_at.get() {
            publication_events.insert(index, conformance::PublicationEvent::GateClosed);
        }
        Trace {
            returned: core::mem::take(&mut *seen.borrow_mut()),
            contexts_dropped: dropped.get(),
            before_teardown_dropped: before_teardown.get(),
            publications: air.sent().len(),
            terminal_evidence: core::mem::take(&mut *evidence.borrow_mut()),
            publication_events,
        }
    }
}

#[test]
fn common_backend_tx_ownership_suite() {
    block_on(async {
        conformance::check(&mut ModelHarness, conformance::Environment::Model)
            .await
            .unwrap();
    });
}

struct AggregateContext {
    ticket: u8,
    buffers: Vec<u8>,
    bodies: Vec<(u8, *const u8)>,
    dropped: Rc<Cell<usize>>,
}

impl ModelContext for AggregateContext {
    type StationConfigureContext<'id> = ();
    type AccessPointConfigureContext<'id> = ();
    type Body = ModelBody;
    type Context<'id> = Self;
}

impl Drop for AggregateContext {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

struct AggregateOwners {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    withdraw: bool,
    initial_pn: u64,
}

impl PortBody<ModelBackend<ModelCaps, AggregateContext>> for AggregateOwners {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend<ModelCaps, AggregateContext>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps, AggregateContext>, ()> {
        let mut terminal_access = resources.terminal_access;
        use oer_ieee80211_port::{AggregateAttempt, AmpduPhase, MpduStartSpacing, Tid};
        let Ok(mut owner) = owner.enable().await else {
            panic!("the model enables")
        };
        let ModelResources {
            mut tickets,
            mut small,
            gate,
            station_pairwise: [slot, _],
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            let ht = oer_ieee80211_wire::phy::HtRate::new(
                HtMcs::new(7).unwrap(),
                oer_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                false,
            )
            .unwrap();
            let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
            let mut station = phase.station();
            let association = ModelKeys::<Station>::begin_association(
                resources.station_association,
                oer_ieee80211_wire::security::Spp::NotNegotiated,
            );
            let key = station
                .install_pairwise_key(
                    &association,
                    slot,
                    IndividualAddress::new(PEER).unwrap(),
                    Ccmp128Key::new(&[7; 16]),
                )
                .into_core_key();
            key.set_last(self.initial_pn);
            let rate = station.ampdu_rate(rate).ok().unwrap();
            let aggregate = station.aggregate(
                IndividualAddress::new(PEER).unwrap(),
                Tid::new(6).unwrap(),
                rate,
                TxProtection::Ccmp(&key),
            );
            let ticket = tickets.remove(0);
            let mut context = AggregateContext {
                ticket: ticket.index(),
                buffers: Vec::new(),
                bodies: Vec::new(),
                dropped: self.dropped.clone(),
            };
            let first = small.remove(0);
            let body = ModelBody(alloc::vec![0; 4]);
            context.buffers.push(first.slot.index());
            context.bodies.push((0, body.0.as_ptr()));
            let mut aggregate = push_subframe(
                aggregate,
                key.numbering(),
                first,
                subframe_frame(0),
                Some(body),
            );
            for sequence in 1..3 {
                let buffer = small.remove(0);
                context.buffers.push(buffer.slot.index());
                // A missing body does not renumber the following body's index.
                let body = (sequence != 1).then(|| ModelBody(alloc::vec![sequence as u8; 4]));
                if let Some(body) = &body {
                    context.bodies.push((sequence as u8, body.0.as_ptr()));
                }
                aggregate = push_subframe(
                    aggregate,
                    key.numbering(),
                    buffer,
                    subframe_frame(sequence),
                    body,
                );
            }
            self.air.hold(WmmAccessCategory::Voice, self.withdraw);
            station
                .submit_ampdu(
                    (AggregateAttempt {
                        context,
                        ticket,
                        aggregate,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                    })
                    .admit(),
                )
                .unwrap();
            if self.withdraw {
                phase.close_gate(gate);
            }
            let event = loop {
                let Ok(Ok(event)) = phase.next_event().await else {
                    panic!("the aggregate ends")
                };
                if matches!(
                    &event,
                    LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(_))
                ) {
                    break event;
                }
            };
            let LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(
                terminal,
            ))) = event
            else {
                unreachable!()
            };
            let oer_ieee80211_port::TerminalParts {
                context,
                ticket,
                payload,
                outcome: completion,
            } = terminal.into_parts(&mut terminal_access);
            let subframes: Vec<_> = payload.into_iter().map(TxMpdu::into_parts).collect();
            assert_eq!(ticket.index(), context.ticket);
            assert_eq!(matches!(completion, TxOutcome::Withdrawn(_)), self.withdraw);
            assert_eq!(subframes.len(), 3);
            assert_eq!(key.last(), self.initial_pn + 3,);
            for (index, (buffer, _)) in subframes.iter().enumerate() {
                assert_eq!(
                    buffer.packet_number(),
                    (!self.withdraw).then_some(self.initial_pn + index as u64 + 1),
                );
            }
            assert_eq!(
                subframes.iter().filter(|(_, body)| body.is_some()).count(),
                if self.withdraw { 2 } else { 0 }
            );
            for ((buffer, _), index) in subframes.iter().zip(&context.buffers) {
                let slot = match buffer {
                    ModelAnyBuffer::Small(buffer) if self.withdraw => buffer.slot.index(),
                    ModelAnyBuffer::SmallSent(buffer) if !self.withdraw => {
                        buffer.buffer.slot.index()
                    }
                    _ => panic!("only published subframes are numbered"),
                };
                assert_eq!(slot, *index);
            }
            for (index, pointer) in &context.bodies {
                let (buffer, fresh_body) = &subframes[usize::from(*index)];
                let body = if self.withdraw {
                    fresh_body.as_ref().unwrap()
                } else {
                    let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                        unreachable!()
                    };
                    mpdu.body().unwrap()
                };
                assert_eq!(body.0.as_ptr(), *pointer);
                assert_eq!(body.0, [*index; 4]);
                assert_eq!(&buffer.memory()[36..40], body.bytes());
            }
        }
        let mut sink = |event| {
            assert!(
                !matches!(event, LowerMacTerminal::TxCompleted(_)),
                "each attempt has one terminal"
            );
            core::ops::ControlFlow::Continue(())
        };
        let Ok(owner) = owner.disable(&mut sink).await else {
            panic!("the model disables")
        };
        Ended::uninstall(owner, (), ())
    }
}

#[test]
fn aggregate_terminal_returns_its_context_and_each_subframes_resources_on_air_or_withdrawal() {
    for (withdraw, initial_pn) in [
        (false, 0),
        (true, 0),
        (false, oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX - 3),
    ] {
        let (backend, air) = ModelBackend::<ModelCaps, AggregateContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        );
        let dropped = Rc::new(Cell::new(0));
        let _port_end = block_on(run_port(
            backend,
            AggregateOwners {
                air,
                dropped: dropped.clone(),
                withdraw,
                initial_pn,
            },
            oer_radio_port::LocalExecution,
        ));
        assert_eq!(dropped.get(), 1);
    }
}

struct FaultContext(Rc<Cell<usize>>);
impl ModelContext for FaultContext {
    type StationConfigureContext<'id> = ();
    type AccessPointConfigureContext<'id> = ();
    type Body = ModelBody;
    type Context<'id> = Self;
}
impl Drop for FaultContext {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct PublicationFault {
    aggregate: bool,
    dropped: Rc<Cell<usize>>,
}
impl PortBody<ModelBackend<ModelCaps, FaultContext>> for PublicationFault {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend<ModelCaps, FaultContext>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps, FaultContext>, ()> {
        let station_association = ModelKeys::<Station>::begin_association(
            resources.station_association,
            oer_ieee80211_wire::security::Spp::NotNegotiated,
        );

        use oer_ieee80211_port::{AggregateAttempt, MpduStartSpacing};
        let mut owner = owner.enable().await.ok().unwrap();
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            let key = phase
                .station()
                .install_pairwise_key(
                    &station_association,
                    slot,
                    IndividualAddress::new(PEER).unwrap(),
                    Ccmp128Key::new(&[7; 16]),
                )
                .into_core_key();
            let last = oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX - 10;
            key.set_last(last);
            let ticket = tickets.remove(0);
            let ticket_index = ticket.index();
            let context = FaultContext(self.dropped.clone());
            let first = small.remove(0);
            let first_index = first.slot.index();
            let body = ModelBody(alloc::vec![0x7a; 4]);
            let first_body = body.0.as_ptr();
            if self.aggregate {
                let ht = oer_ieee80211_wire::phy::HtRate::new(
                    HtMcs::new(7).unwrap(),
                    oer_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap();
                let rate = phase.rate(PhyRate::Ht(ht)).unwrap();
                let mut station = phase.station();
                let rate = station.ampdu_rate(rate).ok().unwrap();
                let aggregate = station.aggregate(
                    IndividualAddress::new(PEER).unwrap(),
                    Tid::new(0).unwrap(),
                    rate,
                    TxProtection::Ccmp(&key),
                );
                let aggregate = push_subframe(
                    aggregate,
                    key.numbering(),
                    first,
                    subframe_frame(0),
                    Some(body),
                );
                let aggregate = push_subframe(
                    aggregate,
                    key.numbering(),
                    small.remove(0),
                    subframe_frame(1),
                    Some(ModelBody(alloc::vec![0x4b; 4])),
                );
                station.held.poison();
                let submitted = station.submit_ampdu(
                    (AggregateAttempt {
                        context,
                        ticket,
                        aggregate,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                    })
                    .admit(),
                );
                assert!(submitted.is_err());
            } else {
                let source = TxSource::Fresh {
                    buffer: first,
                    frame: ClassFrame::new(data_frame(&phase, true)).ok().unwrap(),
                    body: Some(body),
                    protection: TxProtection::Ccmp(key.numbered()),
                };
                let source = <ModelTypes<FaultContext> as TxBufferPool<ModelSmall>>::prepare::<
                    Station,
                    ModelCaps,
                >(source.check().ok().unwrap())
                .ok()
                .unwrap();
                phase.held.poison();
                let submitted = phase.station().submit(
                    (oer_ieee80211_port::Attempt {
                        context,
                        ticket,
                        source,
                        access_category: WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: oer_ieee80211_port::TxOperation::Data,
                    })
                    .admit(),
                );
                assert!(submitted.is_err());
            }
            assert!(phase.next_event().await.is_err());
            assert_eq!(
                self.dropped.get(),
                0,
                "quarantine retains the opaque context before teardown"
            );
            assert!(phase.held.events.is_empty(), "poison invents no terminal");
            assert!(phase.held.air.sent().is_empty());
            assert_eq!(
                key.last(),
                last + if self.aggregate { 2 } else { 1 },
                "PN is reserved by the core before backend submission"
            );
            let quarantined = phase.held.quarantined.items.borrow();
            let commands = phase.held.commands.items.borrow();
            assert!(quarantined.is_empty());
            let (single, aggregate) = match commands.back().unwrap() {
                super::super::Command::Submit(operation) => (Some(operation), None),
                super::super::Command::SubmitAmpdu(operation) => (None, Some(operation)),
                _ => panic!("poisoned submission stays owned in the command queue"),
            };
            let mpdu = match (single, aggregate) {
                (Some(operation), None) => {
                    assert!(!self.aggregate);
                    assert_eq!(operation.operation.ticket().index(), ticket_index);
                    operation.operation.payload()
                }
                (None, Some(operation)) => {
                    assert!(self.aggregate);
                    assert_eq!(operation.operation.ticket().index(), ticket_index);
                    let subframes = &operation.operation.payload().0;
                    assert_eq!(subframes.len(), 2);
                    assert_eq!(subframes[1].mpdu.body.as_ref().unwrap().0, [0x4b; 4]);
                    assert!(
                        subframes
                            .iter()
                            .all(|subframe| subframe.mpdu.published.is_none())
                    );
                    &subframes[0].mpdu
                }
                _ => unreachable!(),
            };
            assert!(mpdu.published.is_none());
            let ModelAnyBuffer::Small(buffer) = &mpdu.buffer else {
                panic!("original allocation")
            };
            assert_eq!(buffer.slot.index(), first_index);
            assert_eq!(mpdu.body.as_ref().unwrap().0.as_ptr(), first_body);
            assert_eq!(mpdu.body.as_ref().unwrap().0, [0x7a; 4]);
        }
        Ended::fail_stop(owner.fail_stop(), ())
    }
}

#[test]
fn publication_fault_keeps_the_complete_single_or_aggregate_until_teardown() {
    for aggregate in [false, true] {
        let dropped = Rc::new(Cell::new(0));
        let (backend, _) = ModelBackend::<ModelCaps, FaultContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        );
        assert!(matches!(
            block_on(run_port(
                backend,
                PublicationFault {
                    aggregate,
                    dropped: dropped.clone()
                },
                oer_radio_port::LocalExecution
            )),
            oer_radio_port::PortEnd::FailStopped { .. }
        ));
        assert_eq!(
            dropped.get(),
            0,
            "dropping the quarantine receipt retains the publication owner"
        );
    }
}
