//! The shared TX ownership suite against the S31 backend, with its one
//! buffer class. The device model holds the BestEffort queue's completions
//! while Voice and Video complete. Publications are identified by the
//! descriptor head each publication's DMA authority names.
//!
//! The S31 port publishes an access category's head as soon as its queue is
//! idle, and a published head is waited for, never withdrawn. CloseGate,
//! Disable and Poison expect the held BestEffort operations unpublished and
//! withdrawn, so only Complete and Refuse run here.
use super::*;
use oer_ieee80211_testing_conformance::{
    self as shared, Class, Classes, End, Operation, PublicationEvent, Resources, ReturnEvidence,
    Returned, Scenario, Trace,
};
use oer_ieee80211_wire::qos::WmmAccessCategory;

/// The scenarios the S31 port can serve as the shared suite states them.
const SCENARIOS: [Scenario; 2] = [Scenario::Complete, Scenario::Refuse];

type Rows = Rc<RefCell<Vec<Returned>>>;
type Evidence = Rc<RefCell<Vec<ReturnEvidence>>>;

/// What the opaque context's value names: the operation and the resource
/// identities recorded before transfer.
type Expected = Rc<RefCell<[Option<(Operation, Resources)>; 4]>>;

struct OwnershipBody {
    scenario: Scenario,
    observed: Rc<RefCell<Observed>>,
    drops: Rc<Cell<usize>>,
    expected: Expected,
    rows: Rows,
    evidence: Evidence,
    before_teardown: Rc<Cell<usize>>,
}

/// An individual data frame; a refused layout leaves no room for the body.
fn frame<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &Esp32s31Phase<'p, 'id, Id, TestDevice, 512>,
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
    let length = MpduLength::new(if refuse { 26 } else { 64 }).expect("length");
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

/// Record an actual return against its context's expected identities.
fn record(
    tag: Tag,
    ticket: u8,
    buffer: u8,
    body: Option<Payload>,
    end: End,
    expected: &Expected,
    rows: &Rows,
) {
    let (operation, resources) = expected.borrow()[tag.0].expect("an admitted operation");
    rows.borrow_mut().push(Returned {
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
        let mut resources = resources;
        {
            let mut phase = owner.phase();
            let best_effort = super::super::super::queue_of(WmmAccessCategory::BestEffort);
            self.observed.borrow_mut().held_queues[best_effort] = true;
            let mut buffers = resources.buffers.into_iter();
            let mut tickets = resources.tickets.into_iter();
            for operation in Classes::One.operations() {
                let ticket = tickets.next().expect("a ticket per operation");
                let buffer = buffers.next().expect("a buffer per operation");
                // Each operation owns its own body allocation.
                let body = Payload(Rc::new(Cell::new(0)));
                let resources = Resources {
                    ticket: ticket.index(),
                    buffer: buffer.index(),
                    class: Class::Small,
                    body: Rc::as_ptr(&body.0) as usize,
                };
                self.expected.borrow_mut()[usize::from(operation.id)] =
                    Some((operation, resources));
                let context = Tag(usize::from(operation.id), Some(self.drops.clone()));
                let checked = TxSource::Fresh {
                    buffer,
                    frame: frame(&phase, self.scenario == Scenario::Refuse),
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
                        self.evidence
                            .borrow_mut()
                            .push(ReturnEvidence::refused(true));
                        record(
                            context,
                            ticket.index(),
                            buffer.index(),
                            body,
                            End::Refused,
                            &self.expected,
                            &self.rows,
                        );
                    }
                }
            }
            if self.scenario == Scenario::Complete {
                let mut take = async |phase: &mut Esp32s31Phase<'_, 'id, Id, TestDevice, 512>| {
                    let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                        TxCompleted::Single(completed),
                    )))) = phase.next_event().await
                    else {
                        panic!("a transmit terminal")
                    };
                    let returned = completed.into_parts(&mut resources.terminal_access);
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
                    record(
                        returned.context,
                        returned.ticket.index(),
                        index,
                        body,
                        end,
                        &self.expected,
                        &self.rows,
                    );
                };
                // Voice and Video complete while BestEffort is held.
                for _ in 0..2 {
                    take(&mut phase).await;
                }
                assert!(
                    self.rows
                        .borrow()
                        .iter()
                        .all(|row| row.operation.category != WmmAccessCategory::BestEffort)
                );
                self.observed.borrow_mut().held_queues[best_effort] = false;
                for _ in 0..2 {
                    take(&mut phase).await;
                }
            }
        }
        let Ok(owner) = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        self.before_teardown.set(self.drops.get());
        radio::Ended::uninstall(owner, (), ())
    }
}

fn exercise(
    scenario: Scenario,
) -> Trace<Vec<Returned>, Vec<ReturnEvidence>, Vec<PublicationEvent>> {
    let (parts, observed, _) = installed(Vec::new());
    let drops = Rc::new(Cell::new(0));
    let rows = Rows::default();
    let evidence = Evidence::default();
    let expected = Expected::default();
    let before_teardown = Rc::new(Cell::new(usize::MAX));
    let body = OwnershipBody {
        scenario,
        observed: observed.clone(),
        drops: drops.clone(),
        expected: expected.clone(),
        rows: rows.clone(),
        evidence: evidence.clone(),
        before_teardown: before_teardown.clone(),
    };
    let Report::Uninstalled { .. } = block_on(run_body(parts, body)) else {
        panic!("clean uninstall")
    };
    let observed = observed.borrow();
    // Each publication's descriptor head names its buffer; the buffer's
    // admitted identity names its ticket.
    let publication_events = observed
        .published_heads
        .iter()
        .map(|head| {
            let buffer = observed
                .buffer_heads
                .iter()
                .position(|candidate| candidate == head)
                .expect("a publication names a port buffer");
            let (_, resources) = expected
                .borrow()
                .iter()
                .flatten()
                .copied()
                .find(|(_, resources)| usize::from(resources.buffer) == buffer)
                .expect("the published buffer belongs to an operation");
            PublicationEvent::Tx(resources.ticket)
        })
        .collect();
    Trace {
        returned: rows.take(),
        contexts_dropped: drops.get(),
        before_teardown_dropped: before_teardown.get(),
        publications: observed.publications,
        terminal_evidence: evidence.take(),
        publication_events,
    }
}

#[test]
fn the_s31_backend_passes_the_shared_ownership_scenarios_it_can_serve() {
    for scenario in SCENARIOS {
        let trace = exercise(scenario);
        assert_eq!(
            shared::verify(scenario, Classes::One, &trace).map_err(|why| (scenario, why)),
            Ok(())
        );
        if scenario == Scenario::Complete {
            assert_eq!(trace.publication_events.len(), 4, "every head identified");
        }
    }
}
