//! The shared TX ownership suite against the S31 backend, with its one
//! buffer class, in the `PublishedHead` mode: the port publishes an idle
//! queue's head at once, so the device model withholds the BestEffort
//! head's completion while Voice and Video complete. Each operation has its
//! own MPDU length, so a publication is identified by the PSDU length of
//! the program handed to the hardware, read live from the device model's
//! publication log; requests, returns and the gate confirmation are placed
//! by the number of publications observed when they happened.
use super::*;
use oer_esp32s31_hal::types::MacLegacyTxProgram;
use oer_ieee80211_testing_conformance::{
    self as shared, Class, Classes, End, Harness, Held, Operation, PublicationEvent, Resources,
    ReturnEvidence, Returned, Scenario, Trace,
};
use oer_ieee80211_wire::qos::WmmAccessCategory;

type Rows = Rc<RefCell<Vec<Returned>>>;
type Evidence = Rc<RefCell<Vec<ReturnEvidence>>>;
type Markers = Rc<RefCell<Vec<(usize, PublicationEvent)>>>;

/// What the opaque context's value names: the operation and the resource
/// identities recorded before transfer.
type Expected = Rc<RefCell<[Option<(Operation, Resources)>; 5]>>;

/// An earlier BestEffort transmission outside the suite's operations: held
/// published, it keeps the suite's BestEffort pair queued unpublished.
const BLOCKER: Operation = Operation {
    id: 4,
    class: Class::Small,
    category: WmmAccessCategory::BestEffort,
};

/// The fixture's shared observations.
#[derive(Clone)]
struct Capture {
    observed: Rc<RefCell<Observed>>,
    /// The device model's live publication log.
    published: Rc<RefCell<Vec<MacLegacyTxProgram>>>,
    drops: Rc<Cell<usize>>,
    expected: Expected,
    rows: Rows,
    evidence: Evidence,
    markers: Markers,
    before_teardown: Rc<Cell<usize>>,
}

impl Capture {
    fn mark(&self, event: PublicationEvent) {
        let at = self.published.borrow().len();
        self.markers.borrow_mut().push((at, event));
    }
    fn hold(&self, held: bool) {
        let best_effort = super::super::super::queue_of(WmmAccessCategory::BestEffort);
        self.observed.borrow_mut().held_queues[best_effort] = held;
    }
    /// Record an actual return against its context's expected identities.
    fn record(&self, tag: Tag, ticket: u8, buffer: u8, body: Option<Payload>, end: End) {
        let (operation, resources) = self.expected.borrow()[tag.0].expect("an admitted operation");
        self.rows.borrow_mut().push(Returned {
            operation,
            expected: resources,
            actual: Resources {
                ticket,
                buffer,
                class: Class::Small,
                body: body.as_ref().map_or(0, |body| Rc::as_ptr(&body.0) as usize),
            },
            end,
        });
    }
    /// Take one port terminal: a transmit return or the gate confirmation.
    fn terminal<'id, Id: radio::SlotIdentity + 'id>(
        &self,
        terminal: LowerMacTerminal<'id, Family<Id>>,
        access: &mut TerminalAccess<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
    ) {
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                let returned = completed.into_parts(access);
                self.mark(PublicationEvent::Returned(returned.ticket.index()));
                self.evidence
                    .borrow_mut()
                    .push(ReturnEvidence::terminal(true, &returned.outcome).expect("evidence"));
                let end = match returned.outcome {
                    TxOutcome::Ended(SingleEnd::Delivered { .. }) => End::Ended,
                    TxOutcome::Withdrawn(_) => End::Withdrawn,
                    other => panic!("unexpected outcome {other:?}"),
                };
                let (buffer, body) = returned.payload.into_parts();
                let (index, body) = match buffer {
                    AnyBuffer::Fresh(fresh) => (fresh.index(), body),
                    AnyBuffer::Sent(sent) => {
                        let index = sent.index();
                        let (_renewed, body) =
                            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::renew(sent);
                        (index, body)
                    }
                };
                self.record(returned.context, returned.ticket.index(), index, body, end);
            }
            LowerMacTerminal::GateClosed(_) => self.mark(PublicationEvent::GateClosed),
            _ => panic!("an unexpected ownership terminal"),
        }
    }
}

/// Records the context destructors run when the body ends: on return, or
/// when a fail-stop cancels it before it returns.
struct Teardown {
    drops: Rc<Cell<usize>>,
    at: Rc<Cell<usize>>,
}
impl Drop for Teardown {
    fn drop(&mut self) {
        self.at.set(self.drops.get());
    }
}

struct OwnershipBody {
    scenario: Scenario,
    /// Submit `BLOCKER` before the suite's operations.
    blocker: bool,
    capture: Capture,
    tickets: Rc<Cell<[u8; 5]>>,
}

/// The MPDU length of operation `id`: distinct per operation.
fn length(id: u8) -> usize {
    64 + usize::from(id)
}

/// An individual data frame of operation `id`; a refused layout leaves no
/// room for the body.
fn frame<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    id: u8,
    refuse: bool,
) -> ClassFrame<'p, 'id, BufferClass<512>, Caps> {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("rate");
    let header = HeaderSpec::new(
        FrameControl::new(oer_ieee80211_wire::frame::DATA_TO_DS).expect("control"),
        0,
        IndividualAddress::new(BSSID).expect("receiver"),
        STATION,
        BSSID,
        0x4560,
        None,
        None,
        None,
    )
    .expect("header");
    let length = MpduLength::new(if refuse { 26 } else { length(id) }).expect("length");
    ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(AckHeader::new(header).expect("ACK"), length).expect("frame"),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        }),
    })
    .ok()
    .expect("class")
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for OwnershipBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let capture = &self.capture;
        let _teardown = Teardown {
            drops: capture.drops.clone(),
            at: capture.before_teardown.clone(),
        };
        let mut access = resources.terminal_access;
        {
            let mut phase = owner.phase();
            capture.hold(true);
            let mut buffers = resources.buffers.into_iter();
            let mut tickets = resources.tickets.into_iter();
            let operations = self
                .blocker
                .then_some(BLOCKER)
                .into_iter()
                .chain(Classes::One.operations());
            for operation in operations {
                let ticket = tickets.next().expect("a ticket per operation");
                let buffer = buffers.next().expect("a buffer per operation");
                let mut admitted = self.tickets.get();
                admitted[usize::from(operation.id)] = ticket.index();
                self.tickets.set(admitted);
                // Each operation owns its own body allocation.
                let body = Payload(Rc::new(Cell::new(0)));
                let resources = Resources {
                    ticket: ticket.index(),
                    buffer: buffer.index(),
                    class: Class::Small,
                    body: Rc::as_ptr(&body.0) as usize,
                };
                capture.expected.borrow_mut()[usize::from(operation.id)] =
                    Some((operation, resources));
                let context = Tag(usize::from(operation.id), Some(capture.drops.clone()));
                let checked = TxSource::Fresh {
                    buffer,
                    frame: frame(&phase, operation.id, self.scenario == Scenario::Refuse),
                    body: Some(body),
                    protection: TxProtection::Open,
                }
                .check();
                match checked {
                    Ok(checked) => {
                        let source =
                            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<
                                Station,
                                Caps,
                            >(checked)
                            .ok()
                            .expect("prepare");
                        phase
                            .station()
                            .submit(
                                Attempt {
                                    context,
                                    ticket,
                                    source,
                                    access_category: operation.category,
                                    protection: Protection::None,
                                    power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                                    coex_operation: TxOperation::Data,
                                }
                                .admit(),
                            )
                            .expect("submit");
                    }
                    Err(refused) => {
                        let TxSource::Fresh { buffer, body, .. } = refused.operation else {
                            panic!("a fresh source refuses fresh")
                        };
                        capture
                            .evidence
                            .borrow_mut()
                            .push(ReturnEvidence::refused(true));
                        capture.record(context, ticket.index(), buffer.index(), body, End::Refused);
                    }
                }
            }
            if self.scenario != Scenario::Refuse {
                let mut next =
                    async |phase: &mut Esp32s31Phase<'_, 'id, Id, TestDevice, 512, Draws>| {
                        let Ok(Ok(LowerMacEvent::Terminal(terminal))) = phase.next_event().await
                        else {
                            panic!("a port terminal")
                        };
                        capture.terminal(terminal, &mut access);
                    };
                // Voice and Video complete while BestEffort is held.
                while capture.rows.borrow().len() < 2 {
                    next(&mut phase).await;
                }
                match self.scenario {
                    Scenario::Complete => {
                        capture.mark(PublicationEvent::Requested);
                        capture.hold(false);
                    }
                    Scenario::CloseGate => {
                        capture.mark(PublicationEvent::Requested);
                        phase.close_gate(resources.gate);
                        // The published head's result arrives after the request.
                        capture.hold(false);
                    }
                    // Disable issues its request below.
                    Scenario::Disable => {}
                    Scenario::Poison => {
                        capture.mark(PublicationEvent::Requested);
                        capture.observed.borrow_mut().fail_poll = Some(Fault::Capacity);
                    }
                    Scenario::Refuse => unreachable!(),
                }
                let gate = |capture: &Capture| {
                    capture
                        .markers
                        .borrow()
                        .iter()
                        .any(|(_, event)| *event == PublicationEvent::GateClosed)
                };
                let all = 4 + usize::from(self.blocker);
                while match self.scenario {
                    Scenario::Complete => capture.rows.borrow().len() < all,
                    Scenario::CloseGate => capture.rows.borrow().len() < all || !gate(capture),
                    _ => false,
                } {
                    next(&mut phase).await;
                }
            }
        }
        let disabling = self.scenario == Scenario::Disable;
        if disabling {
            capture.mark(PublicationEvent::Requested);
        }
        let mut sink = |terminal| {
            capture.terminal(terminal, &mut access);
            core::ops::ControlFlow::Continue(())
        };
        let mut disable = core::pin::pin!(owner.disable(None, &mut sink));
        let mut requested = false;
        let ended = core::future::poll_fn(|cx| {
            let poll = disable.as_mut().poll(cx);
            if disabling && !requested {
                // The first poll issued the request; the published head's
                // result arrives after it.
                requested = true;
                capture.hold(false);
            }
            poll
        })
        .await;
        match ended {
            Ok(owner) => radio::Ended::uninstall(owner, (), ()),
            Err(poisoned) => radio::Ended::fail_stop(poisoned.fail_stop(), ()),
        }
    }
}

/// One run's trace of the suite's operations, and the blocker's ticket.
async fn run(
    scenario: Scenario,
    blocker: bool,
) -> (
    Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>>,
    u8,
) {
    let hardware = Hardware::default();
    let published = hardware.published.clone();
    let (parts, observed, _) = installed_with_hardware(Vec::new(), hardware);
    let capture = Capture {
        observed,
        published,
        drops: Rc::new(Cell::new(0)),
        expected: Expected::default(),
        rows: Rows::default(),
        evidence: Evidence::default(),
        markers: Markers::default(),
        before_teardown: Rc::new(Cell::new(usize::MAX)),
    };
    let tickets = Rc::new(Cell::new([0; 5]));
    let body = OwnershipBody {
        scenario,
        blocker,
        capture: capture.clone(),
        tickets: tickets.clone(),
    };
    let report = run_body(parts, body).await;
    match scenario {
        // The fail-stop may cancel the body before disable returns.
        Scenario::Poison => assert!(matches!(
            report,
            Report::FailStopped { .. } | Report::Abandoned
        )),
        _ => assert!(matches!(report, Report::Uninstalled { .. })),
    }
    // An open legacy PSDU is the MPDU and its FCS; its length names the
    // operation, and the operation's admitted identity names its ticket.
    let [head, tail, voice, video, earlier] = tickets.get();
    let ticket_of = |program: &MacLegacyTxProgram| {
        Classes::One
            .operations()
            .into_iter()
            .chain([BLOCKER])
            .find(|operation| {
                usize::from(program.signal())
                    == length(operation.id) + crate::ordinary_tx::TX_FCS_SIZE
            })
            .map(|operation| tickets.get()[usize::from(operation.id)])
            .expect("a publication names one operation")
    };
    let published: Vec<u8> = capture.published.borrow().iter().map(ticket_of).collect();
    let trace = Trace {
        returned: capture.rows.take(),
        contexts_dropped: capture.drops.get(),
        before_teardown_dropped: capture.before_teardown.get(),
        publications: published.len(),
        terminal_evidence: capture.evidence.take(),
        publication_events: shared::observed_order(published, capture.markers.take()).collect(),
        tickets: [head, tail, voice, video],
    };
    (trace, earlier)
}

struct S31;
impl Harness for S31 {
    const CLASSES: Classes = Classes::One;
    type Returns = Vec<Returned>;
    type Evidence = Vec<ReturnEvidence>;
    type Publications = Vec<PublicationEvent>;
    async fn exercise(
        &mut self,
        scenario: Scenario,
        held: Held,
    ) -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
        // The device model withholds a published head's result; the
        // unpublished mode needs an operation outside the suite's own.
        assert_eq!(held, Held::PublishedHead, "this fixture's mode");
        run(scenario, false).await.0
    }
}

#[test]
fn the_s31_backend_passes_the_shared_ownership_suite_with_a_published_head() {
    let report = block_on(shared::check(
        &mut S31,
        shared::Environment::Model,
        Held::PublishedHead,
    ))
    .expect("the ownership suite");
    assert_eq!(usize::from(report.passed), Scenario::ALL.len());
}

#[test]
fn a_held_earlier_best_effort_transmission_keeps_the_suite_pair_unpublished() {
    use PublicationEvent::{GateClosed, Requested, Returned as Back, Tx};
    // The unpublished arrangement on this port: an earlier BestEffort
    // transmission is published and held, so the suite's pair queues
    // behind it. The blocker is outside the suite's operations, so these
    // observations are checked here rather than by the shared verifier.
    for scenario in [Scenario::CloseGate, Scenario::Disable] {
        let (trace, blocker) = block_on(run(scenario, true));
        let [head, tail, voice, video] = trace.tickets;
        let events = &trace.publication_events;
        let at = |wanted| {
            events
                .iter()
                .position(|event| *event == wanted)
                .unwrap_or_else(|| panic!("{scenario:?}: {wanted:?} observed"))
        };
        let requested = at(Requested);
        // At the request: the blocker, Voice and Video are published and
        // the suite's BestEffort pair is not; the pair is never published.
        for ticket in [blocker, voice, video] {
            assert!(events[..requested].contains(&Tx(ticket)), "{scenario:?}");
        }
        assert!(!events.contains(&Tx(head)) && !events.contains(&Tx(tail)));
        assert!(at(Back(voice)) < requested && at(Back(video)) < requested);
        // The blocker ends with its own terminal before the pair returns
        // withdrawn, in FIFO order; a closed gate confirms after all three.
        assert!(requested < at(Back(blocker)));
        assert!(at(Back(blocker)) < at(Back(head)) && at(Back(head)) < at(Back(tail)));
        if scenario == Scenario::CloseGate {
            assert!(at(Back(tail)) < at(GateClosed));
            assert_eq!(
                events.last(),
                Some(&GateClosed),
                "nothing publishes after it"
            );
        }
        let end = |id: u8| {
            trace
                .returned
                .iter()
                .find(|row| row.operation.id == id)
                .map(|row| (row.end, row.expected == row.actual))
        };
        assert_eq!(end(BLOCKER.id), Some((End::Ended, true)));
        assert_eq!(end(0), Some((End::Withdrawn, true)));
        assert_eq!(end(1), Some((End::Withdrawn, true)));
        assert_eq!(end(2), Some((End::Ended, true)));
        assert_eq!(end(3), Some((End::Ended, true)));
        assert_eq!(trace.publications, 3);
        assert_eq!(
            (trace.contexts_dropped, trace.before_teardown_dropped),
            (5, 5)
        );
    }
}
