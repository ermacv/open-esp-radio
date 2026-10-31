//! The shared EDCA suite against the S31 backend. The P2 driver reports
//! each contention draw to its observation receiver where it selects the
//! publication's parameters; the device model, separately, keeps the AIFSN
//! and backoff of each legacy program the driver hands the hardware. The
//! fixture's entropy is all ones, a held queue is the device model's held
//! completion and a collision is its RTS-error status.
use super::*;
use crate::lower_mac::ContentionDraw;
use oer_ieee80211_testing_conformance::edca::{
    self as contract, Case, Draw, Harness, Scenario, Trace,
};

/// The three operations' VIFs, in submission order.
const ROLES: [VifRole; 3] = [VifRole::Station, VifRole::AccessPoint, VifRole::Station];

/// What the body observed before the port retired.
struct Seen {
    admitted: [u8; 3],
    completed: [u8; 3],
    held_draws: Option<usize>,
}

struct EdcaBody {
    case: Case,
    observed: Rc<RefCell<Observed>>,
    seen: Rc<RefCell<Option<Seen>>>,
}

/// An individual data frame of `role`; a retry case allows one retry.
fn frame<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    role: VifRole,
    transmissions: u8,
) -> ClassFrame<'p, 'id, BufferClass<512>, Caps> {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("rate");
    let (control, receiver, transmitter) = match role {
        VifRole::Station => (oer_ieee80211_wire::frame::DATA_TO_DS, BSSID, STATION),
        // From the DS: the AP's own frame to an associated station.
        VifRole::AccessPoint => (0x0208, PEER_STATION, ACCESS_POINT),
    };
    let header = HeaderSpec::new(
        FrameControl::new(control).expect("control"),
        0,
        IndividualAddress::new(receiver).expect("receiver"),
        transmitter,
        if role == VifRole::Station {
            BSSID
        } else {
            ACCESS_POINT
        },
        0x4560,
        None,
        None,
        None,
    )
    .expect("header");
    ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(
            AckHeader::new(header).expect("ACK"),
            MpduLength::new(64).expect("length"),
        )
        .expect("frame"),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(transmissions).expect("a transmission"),
            protection: Protection::None,
        }),
    })
    .ok()
    .expect("class")
}

/// A station the AP addresses; not an address of either VIF.
const PEER_STATION: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x77];

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for EdcaBody {
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
            let case = self.case;
            if case.scenario != Scenario::Defaults {
                let station = SupportedEdca::new(contract::parameters(VifRole::Station, false))
                    .expect("station table");
                let ap = SupportedEdca::new(contract::parameters(VifRole::AccessPoint, false))
                    .expect("AP table");
                assert_eq!(phase.station().set_edca(station), station);
                assert_eq!(phase.access_point().set_edca(ap), ap);
            }
            let held = case.scenario == Scenario::QueuedUpdate;
            let queue = super::super::super::queue_of(case.category);
            self.observed.borrow_mut().held_queues[queue] = held;
            let transmissions = if case.scenario == Scenario::Retry {
                2
            } else {
                1
            };
            let mut tickets = resources.tickets.into_iter();
            let mut buffers = resources.buffers.into_iter();
            let mut admitted = [0; 3];
            for (index, role) in ROLES.into_iter().enumerate() {
                let ticket = tickets.next().expect("a ticket per operation");
                let buffer = buffers.next().expect("a buffer per operation");
                admitted[index] = ticket.index();
                let frame = frame(&phase, role, transmissions);
                macro_rules! submit {
                    ($role:ty, $interface:ident) => {{
                        let checked = TxSource::Fresh {
                            buffer,
                            frame,
                            body: None,
                            protection: TxProtection::Open,
                        }
                        .check()
                        .ok()
                        .expect("an open fresh source");
                        let source =
                            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<
                                $role,
                                Caps,
                            >(checked)
                            .ok()
                            .expect("prepare");
                        phase
                            .$interface()
                            .submit(
                                Attempt {
                                    context: Tag(index, None),
                                    ticket,
                                    source,
                                    access_category: case.category,
                                    protection: Protection::None,
                                    power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                                    coex_operation: TxOperation::Data,
                                }
                                .admit(),
                            )
                            .expect("submit");
                    }};
                }
                match role {
                    VifRole::Station => submit!(Station, station),
                    VifRole::AccessPoint => submit!(AccessPoint, access_point),
                }
            }
            let held_draws = if held {
                for _ in 0..6 {
                    barriers::yield_once().await;
                }
                let draws = self.observed.borrow().draws.borrow().len();
                let ap = SupportedEdca::new(contract::parameters(VifRole::AccessPoint, true))
                    .expect("updated AP table");
                assert_eq!(phase.access_point().set_edca(ap), ap);
                for _ in 0..6 {
                    barriers::yield_once().await;
                }
                assert_eq!(
                    self.observed.borrow().draws.borrow().len(),
                    draws,
                    "a VIF update neither republishes the head nor launches the tail"
                );
                self.observed.borrow_mut().held_queues[queue] = false;
                Some(draws)
            } else {
                None
            };
            let mut completed = [0; 3];
            for index in &mut completed {
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(terminal),
                )))) = phase.next_event().await
                else {
                    panic!("a transmit terminal")
                };
                let returned = terminal.into_parts(&mut resources.terminal_access);
                assert!(
                    matches!(
                        returned.outcome,
                        TxOutcome::Ended(SingleEnd::Delivered { .. })
                    ),
                    "every operation is delivered"
                );
                *index = returned.ticket.index();
                let (buffer, _) = returned.payload.into_parts();
                let AnyBuffer::Sent(sent) = buffer else {
                    panic!("completion returns a published MPDU")
                };
                let _renewed = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::renew(sent);
            }
            *self.seen.borrow_mut() = Some(Seen {
                admitted,
                completed,
                held_draws,
            });
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

/// The driver's draws, each checked against the program it handed the
/// hardware: the same publication count, and per publication the selected
/// record's AIFSN and the drawn backoff.
fn programmed(observed: &Observed) -> Vec<Draw> {
    let draws: Vec<ContentionDraw> = observed.draws.borrow().clone();
    assert_eq!(
        draws.len(),
        observed.publications,
        "one draw per publication"
    );
    assert_eq!(observed.aifsns.len(), draws.len());
    for ((draw, aifsn), backoff) in draws.iter().zip(&observed.aifsns).zip(&observed.backoffs) {
        assert_eq!(
            *aifsn,
            draw.parameters.wmm.aifsn.get(),
            "the program waits the selected AIFSN"
        );
        assert_eq!(*backoff, draw.backoff_slots, "the program counts the draw");
    }
    draws
        .into_iter()
        .map(|draw| Draw {
            role: draw.role,
            category: draw.category,
            parameters: draw.parameters,
            window_slots: draw.window_slots,
            backoff_slots: draw.backoff_slots,
        })
        .collect()
}

struct S31;
impl Harness for S31 {
    type Draws = Vec<Draw>;
    async fn exercise(&mut self, case: Case) -> Trace<Self::Draws> {
        let statuses = if case.scenario == Scenario::Retry {
            // The station head collides once, then every publication is
            // acknowledged.
            std::vec![(1, 1), (0, 0), (0, 0), (0, 0)]
        } else {
            std::vec![(0, 0); 3]
        };
        let (parts, observed, _) =
            installed_with_entropy(statuses, Hardware::default(), || u32::MAX);
        let seen = Rc::new(RefCell::new(None));
        let body = EdcaBody {
            case,
            observed: observed.clone(),
            seen: seen.clone(),
        };
        let installed = radio::with_local_slot(async |claim| {
            let mut slot = claim.place();
            let backend = Backend::new(parts).ok().expect("valid construction");
            let storage = backend.storage_for_test();
            let registered = slot.register(backend).ok().expect("fresh local slot");
            let end = radio::LocalExecution
                .scope(crate::port::Registration::guarded(registered), |frontend| {
                    radio::run_port(frontend, body)
                })
                .await;
            assert!(
                matches!(end, radio::PortEnd::Uninstalled { .. }),
                "clean uninstall"
            );
            storage.installed_edca()
        })
        .await;
        let seen = seen.borrow_mut().take().expect("the body completed");
        let observed = observed.borrow();
        Trace {
            draws: programmed(&observed),
            installed: installed.map(|set| set.expect("the driver installed a table")),
            admitted: seen.admitted,
            completed: seen.completed,
            held_draws: seen.held_draws,
        }
    }
}

#[test]
fn the_s31_backend_passes_the_shared_edca_suite() {
    let report = block_on(contract::check(&mut S31, Environment::Model)).expect("the EDCA suite");
    assert_eq!(usize::from(report.passed), Case::ALL.len());
}
