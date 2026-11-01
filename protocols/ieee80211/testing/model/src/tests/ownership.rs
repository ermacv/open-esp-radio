use super::*;
use oer_ieee80211_testing_conformance::{
    self as conformance, Class, End, Harness, Held, Operation, PublicationEvent, Resources,
    Scenario, Trace,
};

// A fixture-local installation brand; this harness never creates a core epoch.
struct InstallBrand<Id>(PhantomData<fn(Id) -> Id>);

// Deliberately neither Copy nor Clone, and branded by the installation.
struct Context<Id> {
    epoch: Option<InstallBrand<Id>>,
    operation: Operation,
    resources: Resources,
    dropped: Rc<Cell<usize>>,
}
struct Contexts;
impl ModelContext for Contexts {
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type Body = ModelBody;
    type OwnedContext<Id: SlotIdentity> = Context<Id>;
    type Context<'p, Id: SlotIdentity + 'p> = Context<Id>;
}
impl<Id> Drop for Context<Id> {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
type Backend<Id> = ModelBackend<Id, ModelCaps, Contexts>;

struct Operations {
    air: Rc<ModelAir>,
    dropped: Rc<Cell<usize>>,
    before_teardown: Rc<Cell<usize>>,
    seen: Rc<RefCell<Vec<conformance::Returned>>>,
    scenario: Scenario,
    held: Held,
    evidence: Rc<RefCell<Vec<conformance::ReturnEvidence>>>,
    /// Requests, returns and the gate confirmation, each with the number of
    /// publications observed when it happened.
    markers: Rc<RefCell<Vec<(usize, PublicationEvent)>>>,
    tickets: Rc<Cell<[u8; 4]>>,
}

impl Operations {
    fn mark(&self, event: PublicationEvent) {
        self.markers
            .borrow_mut()
            .push((self.air.sent().len(), event));
    }
    /// Hold or release BestEffort as the fixture mode names.
    fn hold(&self, held: bool) {
        match self.held {
            Held::Unpublished => self.air.hold(WmmAccessCategory::BestEffort, held),
            Held::PublishedHead => self
                .air
                .hold_completion(WmmAccessCategory::BestEffort, held),
        }
    }
}

fn observe<'id, Id: SlotIdentity + 'id>(
    context: Context<Id>,
    ticket: Slot<'id, ModelTickets, ModelPool<Id, 1>>,
    buffer: ModelAnyBuffer<'id, Backend<Id>>,
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
            let (buffer, body) = <ModelTypes<Backend<Id>> as TxBufferPool<ModelSmall>>::renew(mpdu);
            (Class::Small, buffer.slot.index(), body)
        }
        ModelAnyBuffer::LargeSent(mpdu) => {
            assert!(body.is_none());
            let (buffer, body) = <ModelTypes<Backend<Id>> as TxBufferPool<ModelLarge>>::renew(mpdu);
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

fn returned<'id, Id: SlotIdentity + 'id>(
    event: LowerMacEvent<'id, ModelTypes<Backend<Id>>>,
    seen: &RefCell<Vec<conformance::Returned>>,
    terminal_access: &mut oer_ieee80211_contracts_port::TerminalAccess<
        'id,
        ModelTickets,
        ModelPool<Id, 1>,
    >,
    evidence: &RefCell<Vec<conformance::ReturnEvidence>>,
    markers: &RefCell<Vec<(usize, PublicationEvent)>>,
    air: &ModelAir,
) {
    match event {
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal))) => {
            let oer_ieee80211_contracts_port::TerminalParts {
                context,
                ticket,
                outcome: completion,
                payload,
                ..
            } = terminal.into_parts(terminal_access);
            let (buffer, body) = payload.into_parts();
            markers
                .borrow_mut()
                .push((air.sent().len(), PublicationEvent::Returned(ticket.index())));
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
            markers
                .borrow_mut()
                .push((air.sent().len(), PublicationEvent::GateClosed));
        }
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(_))) => {
            panic!("single submission must return Single")
        }
        _ => {}
    }
}

impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Operations {
    type Output = ();
    async fn run<'a, 'id>(
        self,
        owner: Owner<'a, 'id, Id, Backend<Id>, Disabled>,
        resources: ModelResources<'id, Backend<Id>>,
    ) -> Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'a,
    {
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
            self.hold(true);
            for operation in conformance::Classes::Two.operations() {
                let ticket = tickets.remove(0);
                let mut admitted = self.tickets.get();
                admitted[usize::from(operation.id)] = ticket.index();
                self.tickets.set(admitted);
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
                            <ModelTypes<Backend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
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
                            <ModelTypes<Backend<Id>> as TxBufferPool<ModelLarge>>::prepare::<
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
                                (oer_ieee80211_contracts_port::Attempt {
                                    context,
                                    ticket,
                                    source,
                                    access_category: operation.category,
                                    protection: Protection::None,
                                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                    coex_operation: oer_ieee80211_contracts_port::TxOperation::Data,
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
                        &self.markers,
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
                    Scenario::Complete => {
                        self.mark(PublicationEvent::Requested);
                        self.hold(false);
                    }
                    Scenario::CloseGate => {
                        self.mark(PublicationEvent::Requested);
                        phase.close_gate(gate);
                        // A published head's result arrives after the request.
                        self.hold(false);
                    }
                    // Disable issues its request below.
                    Scenario::Disable => {}
                    Scenario::Poison => {
                        self.mark(PublicationEvent::Requested);
                        self.air.poison();
                    }
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
                            &self.markers,
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
                &self.markers,
                &self.air,
            );
            core::ops::ControlFlow::Continue(())
        };
        let disabling = self.scenario == Scenario::Disable;
        if disabling {
            self.mark(PublicationEvent::Requested);
        }
        let mut disable = core::pin::pin!(owner.disable(None, &mut sink));
        let mut requested = false;
        let ended = core::future::poll_fn(|cx| {
            let poll = disable.as_mut().poll(cx);
            if disabling && !requested {
                // The first poll issued the request; a published head's
                // result arrives after it.
                requested = true;
                self.hold(false);
            }
            poll
        })
        .await;
        match ended {
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
    const CLASSES: conformance::Classes = conformance::Classes::Two;
    type Returns = Vec<conformance::Returned>;
    type Evidence = Vec<conformance::ReturnEvidence>;
    type Publications = Vec<conformance::PublicationEvent>;
    async fn exercise(
        &mut self,
        scenario: Scenario,
        held: Held,
    ) -> Trace<Self::Returns, Self::Evidence, Self::Publications> {
        let (backend, air) = Fixture::<ModelCaps, Contexts>::new(
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
        let markers = Rc::new(RefCell::new(Vec::new()));
        let tickets = Rc::new(Cell::new([0; 4]));
        let after_receipt_dropped = Cell::new(usize::MAX);
        let _port_end = fixture::inspect_model(
            backend,
            Operations {
                air: air.clone(),
                dropped: dropped.clone(),
                before_teardown: before_teardown.clone(),
                seen: seen.clone(),
                scenario,
                held,
                evidence: evidence.clone(),
                markers: markers.clone(),
                tickets: tickets.clone(),
            },
            |_| after_receipt_dropped.set(dropped.get()),
        )
        .await;
        let publication_events = conformance::observed_order(
            air.sent().iter().map(|frame| frame.ticket),
            markers.take(),
        )
        .collect();
        Trace {
            returned: core::mem::take(&mut *seen.borrow_mut()),
            contexts_dropped: after_receipt_dropped.get(),
            before_teardown_dropped: before_teardown.get(),
            publications: air.sent().len(),
            terminal_evidence: core::mem::take(&mut *evidence.borrow_mut()),
            publication_events,
            tickets: tickets.get(),
        }
    }
}

#[test]
fn common_backend_tx_ownership_suite() {
    // The model holds BestEffort before publication, or publishes its head
    // and withholds the result.
    for held in Held::ALL {
        block_on(async {
            conformance::check(&mut ModelHarness, conformance::Environment::Model, held)
                .await
                .unwrap();
        });
    }
}

struct AggregateContext {
    ticket: u8,
    buffers: Vec<u8>,
    bodies: Vec<(u8, *const u8)>,
    dropped: Rc<Cell<usize>>,
}

impl ModelContext for AggregateContext {
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type Body = ModelBody;
    type OwnedContext<Id: SlotIdentity> = Self;
    type Context<'p, Id: SlotIdentity + 'p> = Self;
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

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id, ModelCaps, AggregateContext>>
    for AggregateOwners
{
    type Output = ();
    async fn run<'a, 'id>(
        self,
        owner: Owner<'a, 'id, Id, ModelBackend<Id, ModelCaps, AggregateContext>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id, ModelCaps, AggregateContext>>,
    ) -> Ended<Id, ModelBackend<Id, ModelCaps, AggregateContext>, ()>
    where
        ModelBackend<Id, ModelCaps, AggregateContext>: 'id,
        Id: 'id,
        'id: 'a,
    {
        let mut terminal_access = resources.terminal_access;
        use oer_ieee80211_contracts_port::{AggregateAttempt, AmpduPhase, MpduStartSpacing, Tid};
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
            let association = ModelKeys::<ModelBackend<Id, ModelCaps, AggregateContext>, Station>::begin_association(
                resources.station_association,
                oer_ieee80211_wire::security::Spp::NotNegotiated,
            );
            let key = installed_station_pairwise(
                &mut phase,
                &association,
                slot,
                IndividualAddress::new(PEER).unwrap(),
                Ccmp128Key::new(&[7; 16]),
            )
            .await
            .expect("the PTK installation terminal")
            .into_core_key();
            let mut station = phase.station();
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
            let oer_ieee80211_contracts_port::TerminalParts {
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
        let Ok(owner) = owner.disable(None, &mut sink).await else {
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
        let (backend, air) = Fixture::<ModelCaps, AggregateContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        );
        let dropped = Rc::new(Cell::new(0));
        let _port_end = block_on(run_model(
            backend,
            AggregateOwners {
                air,
                dropped: dropped.clone(),
                withdraw,
                initial_pn,
            },
        ));
        assert_eq!(dropped.get(), 1);
    }
}

struct FaultContext(Rc<Cell<usize>>);
impl ModelContext for FaultContext {
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type Body = ModelBody;
    type OwnedContext<Id: SlotIdentity> = Self;
    type Context<'p, Id: SlotIdentity + 'p> = Self;
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
impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id, ModelCaps, FaultContext>>
    for PublicationFault
{
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id, ModelCaps, FaultContext>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id, ModelCaps, FaultContext>>,
    ) -> Ended<Id, ModelBackend<Id, ModelCaps, FaultContext>, ()>
    where
        ModelBackend<Id, ModelCaps, FaultContext>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let station_association =
            ModelKeys::<ModelBackend<Id, ModelCaps, FaultContext>, Station>::begin_association(
                resources.station_association,
                oer_ieee80211_wire::security::Spp::NotNegotiated,
            );

        use oer_ieee80211_contracts_port::{AggregateAttempt, MpduStartSpacing};
        let mut owner = owner.enable().await.ok().unwrap();
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        {
            let mut phase = owner.phase();
            let key = installed_station_pairwise(
                &mut phase,
                &station_association,
                slot,
                IndividualAddress::new(PEER).unwrap(),
                Ccmp128Key::new(&[7; 16]),
            )
            .await
            .expect("the PTK installation terminal")
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
                let source =
                    <ModelTypes<ModelBackend<Id, ModelCaps, FaultContext>> as TxBufferPool<
                        ModelSmall,
                    >>::prepare::<Station, ModelCaps>(
                        source.check().ok().unwrap()
                    )
                    .ok()
                    .unwrap();
                phase.held.poison();
                let submitted = phase.station().submit(
                    (oer_ieee80211_contracts_port::Attempt {
                        context,
                        ticket,
                        source,
                        access_category: WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: oer_ieee80211_contracts_port::TxOperation::Data,
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
            let quarantined = phase.held.quarantined.items.inspect();
            let commands = phase.held.commands.items.inspect();
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
        let (backend, _) = Fixture::<ModelCaps, FaultContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
        );
        assert!(matches!(
            block_on(fixture::inspect_model(
                backend,
                PublicationFault {
                    aggregate,
                    dropped: dropped.clone()
                },
                |report| {
                    assert!(matches!(report, Report::Stopped(_)));
                    assert_eq!(
                        dropped.get(),
                        0,
                        "dropping the quarantine receipt retains the publication owner"
                    );
                },
            )),
            Report::Stopped(_)
        ));
        assert_eq!(
            dropped.get(),
            1,
            "the entire model world ends only after P2 registration and all body loans"
        );
    }
}

plain_context!(AggregateContext);
plain_context!(FaultContext);

impl<Id: SlotIdentity> oer_radio_port::ContextProjection<Id> for Context<Id> {
    type View<'a, 'p>
        = &'a Self
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Parts<'p>
        = Self
    where
        Self: 'p,
        Id: 'p;
    fn view<'a, 'p>(&'a self, _: oer_radio_port::ContextLoan<'p, Id>) -> Self::View<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        self
    }
    fn into_parts<'p>(self, _: oer_radio_port::ContextLoan<'p, Id>) -> Self::Parts<'p>
    where
        Self: 'p,
        Id: 'p,
    {
        self
    }
}
impl<'p, Id: SlotIdentity + 'p> LoanedContext<'p, Id> for Context<Id> {
    type Owned = Self;
    fn enclose(self) -> oer_radio_port::PortContext<'p, Id, Self> {
        oer_radio_port::PortContext::new(self)
    }
    fn restore(context: oer_radio_port::PortContext<'p, Id, Self>) -> Self {
        context.into_parts()
    }
}
