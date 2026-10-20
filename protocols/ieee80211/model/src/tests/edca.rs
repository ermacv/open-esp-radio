use super::*;
use oer_ieee80211_port::conformance::{
    self,
    edca::{self as contract, Case, Draw, Scenario, Trace},
};

type Recorded = Rc<RefCell<Option<Trace<Vec<Draw>>>>>;
struct Exercise {
    case: Case,
    trace: Recorded,
}

fn submit_role<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    role: VifRole,
    category: WmmAccessCategory,
    ticket: Slot<'id, ModelTickets, ModelPool<1>>,
    buffer: ModelBuffer<'id, ModelSmall>,
) {
    let frame = ClassFrame::new(data_frame(phase, false)).ok().unwrap();
    macro_rules! submit {
        ($interface:ident) => {{
            let source = prepare_model(
                TxSource::Fresh {
                    buffer,
                    frame,
                    body: None,
                    protection: TxProtection::Open,
                }
                .check()
                .ok()
                .unwrap(),
            )
            .ok()
            .unwrap();
            phase
                .$interface()
                .submit(
                    (oer_ieee80211_port::Attempt {
                        context: (),
                        ticket,
                        source,
                        access_category: category,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: oer_ieee80211_port::TxOperation::Data,
                    })
                    .admit(),
                )
                .unwrap();
        }};
    }
    match role {
        VifRole::Station => submit!(station),
        VifRole::AccessPoint => submit!(access_point),
    }
}

impl Work for Exercise {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            mut terminal_access,
            ..
        } = resources;
        let air = phase.held.air.clone();
        air.set_backoff_entropy(u32::MAX);
        if self.case.scenario != Scenario::Defaults {
            let station =
                SupportedEdca::new(contract::parameters(VifRole::Station, false)).unwrap();
            let ap = SupportedEdca::new(contract::parameters(VifRole::AccessPoint, false)).unwrap();
            assert_eq!(phase.station().set_edca(station), station);
            assert_eq!(phase.access_point().set_edca(ap), ap);
        }
        let held = self.case.scenario == Scenario::QueuedUpdate;
        air.hold_completion(self.case.category, held);
        if self.case.scenario == Scenario::Retry {
            air.answer(AirAnswer::Collision);
        }
        let mut admitted = [0; 3];
        for (index, role) in [VifRole::Station, VifRole::AccessPoint, VifRole::Station]
            .into_iter()
            .enumerate()
        {
            let ticket = tickets.remove(0);
            admitted[index] = ticket.index();
            submit_role(phase, role, self.case.category, ticket, small.remove(0));
        }
        let held_draws = if held {
            for _ in 0..6 {
                yield_now().await;
            }
            let draws = air.contentions().len();
            let ap = SupportedEdca::new(contract::parameters(VifRole::AccessPoint, true)).unwrap();
            assert_eq!(phase.access_point().set_edca(ap), ap);
            for _ in 0..6 {
                yield_now().await;
            }
            assert_eq!(
                air.contentions().len(),
                draws,
                "a VIF update cannot republish the head or launch the tail"
            );
            air.hold_completion(self.case.category, false);
            Some(draws)
        } else {
            None
        };
        let mut completed = [0; 3];
        for index in &mut completed {
            let (ticket, buffer) = completion(phase, &mut terminal_access).await;
            assert!(
                matches!(buffer, ModelAnyBuffer::SmallSent(_)),
                "completion returns a published MPDU"
            );
            *index = ticket.index();
        }
        self.trace.replace(Some(Trace {
            draws: air
                .contentions()
                .into_iter()
                .map(|draw| Draw {
                    role: draw.role,
                    category: draw.access_category,
                    parameters: draw.parameters,
                    window_slots: draw.window_slots,
                    backoff_slots: draw.backoff_slots,
                })
                .collect(),
            installed: [air.edca(VifRole::Station), air.edca(VifRole::AccessPoint)],
            admitted,
            completed,
            held_draws,
        }));
    }
}

struct Harness;
impl contract::Harness for Harness {
    type Draws = Vec<Draw>;
    async fn exercise(&mut self, case: Case) -> Trace<Self::Draws> {
        let recorded = Rc::new(RefCell::new(None));
        let (backend, _) = backend();
        assert!(matches!(
            run_port(
                backend,
                Body(Exercise {
                    case,
                    trace: recorded.clone()
                })
            )
            .await,
            oer_radio_port::PortEnd::Uninstalled { .. }
        ));
        recorded
            .take()
            .expect("the fixture captured actual draws and owners")
    }
}

#[test]
fn per_vif_edca_passes_the_shared_port_conformance_cases_for_every_ac() {
    let report = block_on(contract::check(
        &mut Harness,
        conformance::Environment::Model,
    ))
    .expect("the model satisfies shared EDCA conformance");
    assert_eq!(usize::from(report.passed), Case::ALL.len());
    assert_eq!(report.model_only, 0);
}

#[test]
fn supported_edca_refuses_whole_records_without_clamping() {
    enum LimitedTxop {}
    impl Ieee80211Capabilities for LimitedTxop {
        type SppScope = <ModelCaps as Ieee80211Capabilities>::SppScope;
        const CAPS: LowerMacCapabilities = LowerMacCapabilities {
            edca_max_txop_limit_units_32_us: 94,
            ..ModelCaps::CAPS
        };
    }
    use oer_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
    let standard = EdcaParameterSet::STANDARD_DEFAULT;
    let wmm = standard.wmm();
    for ac in WmmAccessCategory::ALL {
        let baseline = wmm.access_category(ac);
        for invalid in [
            WmmAcParameters {
                aifsn: 16,
                ..baseline
            },
            WmmAcParameters {
                ecw_min: 11,
                ecw_max: 10,
                ..baseline
            },
            WmmAcParameters {
                ecw_max: 11,
                ..baseline
            },
            WmmAcParameters {
                txop_limit_units_32_us: 95,
                ..baseline
            },
        ] {
            let mut records = WmmAccessCategory::ALL.map(|category| wmm.access_category(category));
            *ac.select_mut(&mut records) = invalid;
            let parameters = EdcaParameterSet::from_wmm(WmmParameterSet::new(0, false, records));
            assert!(SupportedEdca::<LimitedTxop>::new(parameters).is_none());
        }
        let mut lifetimes = [500; 4];
        *ac.select_mut(&mut lifetimes) = 501;
        assert!(SupportedEdca::<ModelCaps>::new(EdcaParameterSet::new(wmm, lifetimes)).is_none());
    }
    let retained = EdcaParameterSet::new(wmm, [0, 1, 200, 500]);
    assert_eq!(
        SupportedEdca::<LimitedTxop>::new(retained).unwrap().get(),
        retained
    );
    let wide = contract::parameters(VifRole::AccessPoint, true);
    assert_eq!(
        SupportedEdca::<ModelCaps>::new(wide).unwrap().get(),
        wide,
        "TXOP is carried without narrowing to HE signal-field limits"
    );
}
