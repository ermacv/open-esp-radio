use super::*;
use oer_protocols_ieee80211_testing_conformance::{
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
    terminal_access: &mut oer_protocols_ieee80211_contracts_port::TerminalAccess<
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
            let oer_protocols_ieee80211_contracts_port::TerminalParts {
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
        LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Arena(_))) => {
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
                                (oer_protocols_ieee80211_contracts_port::Attempt {
                                    context,
                                    ticket,
                                    source,
                                    access_category: operation.category,
                                    protection: Protection::None,
                                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                    coex_operation:
                                        oer_protocols_ieee80211_contracts_port::TxOperation::Data,
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
            std::rc::Rc::new(oer_portable_time_virtual::VirtualClock::<8>::new()),
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
            air.sent().iter().filter_map(|frame| frame.ticket),
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
                oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
            );

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
            let last = oer_protocols_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX - 10;
            key.set_last(last);
            let ticket = tickets.remove(0);
            let ticket_index = ticket.index();
            let context = FaultContext(self.dropped.clone());
            let first = small.remove(0);
            let first_index = first.slot.index();
            let body = ModelBody(alloc::vec![0x7a; 4]);
            let first_body = body.0.as_ptr();
            {
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
                    (oer_protocols_ieee80211_contracts_port::Attempt {
                        context,
                        ticket,
                        source,
                        access_category: WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: oer_protocols_ieee80211_contracts_port::TxOperation::Data,
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
                last + 1,
                "PN is reserved by the core before backend submission"
            );
            let quarantined = phase.held.quarantined.items.inspect();
            let commands = phase.held.commands.items.inspect();
            assert!(quarantined.is_empty());
            // A poisoned single stays owned in the command queue.
            let Some(super::super::Command::Submit(operation)) = commands.back() else {
                panic!("poisoned submission stays owned");
            };
            assert_eq!(operation.operation.ticket().index(), ticket_index);
            let mpdu = operation.operation.payload();
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
fn publication_fault_keeps_the_complete_single_until_teardown() {
    {
        let dropped = Rc::new(Cell::new(0));
        let (backend, _) = Fixture::<ModelCaps, FaultContext>::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            std::rc::Rc::new(oer_portable_time_virtual::VirtualClock::<8>::new()),
        );
        assert!(matches!(
            block_on(fixture::inspect_model(
                backend,
                PublicationFault {
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

plain_context!(FaultContext);

impl<Id: SlotIdentity> oer_protocols_radio_port::ContextProjection<Id> for Context<Id> {
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
    fn view<'a, 'p>(
        &'a self,
        _: oer_protocols_radio_port::ContextLoan<'p, Id>,
    ) -> Self::View<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        self
    }
    fn into_parts<'p>(self, _: oer_protocols_radio_port::ContextLoan<'p, Id>) -> Self::Parts<'p>
    where
        Self: 'p,
        Id: 'p,
    {
        self
    }
}
impl<'p, Id: SlotIdentity + 'p> LoanedContext<'p, Id> for Context<Id> {
    type Owned = Self;
    fn enclose(self) -> oer_protocols_radio_port::PortContext<'p, Id, Self> {
        oer_protocols_radio_port::PortContext::new(self)
    }
    fn restore(context: oer_protocols_radio_port::PortContext<'p, Id, Self>) -> Self {
        context.into_parts()
    }
}
