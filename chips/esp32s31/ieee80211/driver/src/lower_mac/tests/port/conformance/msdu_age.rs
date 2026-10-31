//! The shared admission-age suite against the S31 backend: an MSDU admitted
//! with a sub-TU lifetime expires before its publication or between retries,
//! and an acknowledgement of a hardware-owned publication wins over its age.
//! Re-formation and BA-window delivery require `AmpduPhase`, which the S31
//! port does not expose; those cases are unsupported here, not model-only.
use super::*;
use oer_ieee80211_testing_conformance::msdu_age::{Case, End, Trace, verify};

/// The cases the ordinary S31 port serves.
const CASES: [Case; 3] = [
    Case::BeforePublication,
    Case::BetweenRetries,
    Case::HardwareOwned,
];

/// Statuses and retry budget of the case's one MPDU.
fn plan(case: Case) -> (Vec<(u8, u8)>, u8) {
    match case {
        Case::BetweenRetries => (std::vec![(5, 0)], 2),
        _ => (std::vec![(0, 0)], 1),
    }
}

fn coordinate(at: Option<Instant>) -> Ieee80211Instant {
    Ieee80211Instant::from_micros(at.map_or(0, |at| at.as_micros()))
}

struct AgeBody {
    case: Case,
    budget: u8,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    seen: Rc<RefCell<Option<Trace<Vec<End>>>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for AgeBody {
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
            let table = EdcaParameterSet::new(
                EdcaParameterSet::STANDARD_DEFAULT.wmm(),
                [Case::LIFETIME; 4],
            );
            let _applied = phase
                .station()
                .set_edca(SupportedEdca::new(table).expect("a sub-TU lifetime"));
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            let owners = [ticket.index(), buffer.index(), buffer.index()];
            let admitted = coordinate(Some(self.timer.now()));
            self.observed.borrow_mut().hold_completion = self.case != Case::BeforePublication;
            submit_mpdu_budget(&mut phase, buffer, ticket, None, None, self.budget);
            if self.case == Case::BeforePublication {
                // Before P2 runs: the queued MPDU ages past its lifetime.
                self.timer.0.advance(Case::LIFETIME).expect("virtual time");
            } else {
                for _ in 0..4 {
                    barriers::yield_once().await;
                }
                assert_eq!(self.observed.borrow().publications, 1, "hardware owns it");
                self.timer.0.advance(Case::LIFETIME).expect("virtual time");
                self.observed.borrow_mut().hold_completion = false;
            }
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Single(completed),
            )))) = phase.next_event().await
            else {
                panic!("the MPDU's terminal")
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            let end = match returned.outcome {
                TxOutcome::Expired { msdus } => End::Expired(msdus),
                TxOutcome::Ended(SingleEnd::Delivered { .. }) => End::Delivered,
                _ => panic!("the age fixture expects a known terminal"),
            };
            let (buffer, _) = returned.payload.into_parts();
            let (index, at) = match &buffer {
                AnyBuffer::Fresh(fresh) => (fresh.index(), fresh.admitted_at()),
                AnyBuffer::Sent(sent) => (sent.index(), sent.admitted_at()),
            };
            *self.seen.borrow_mut() = Some(Trace {
                ends: std::vec![end],
                publications: self.observed.borrow().publications,
                expired_msdus: 0,
                admitted: [admitted; 2],
                returned: [coordinate(at); 2],
                owners,
                returned_owners: [returned.ticket.index(), index, index],
                backend_bars: 0,
                core_bar: None,
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

/// Run one case and read the separate expiry counter from the retired port.
fn exercise(case: Case) -> Trace<Vec<End>> {
    let (statuses, budget) = plan(case);
    let (parts, observed, timer) = installed(statuses);
    let seen = Rc::new(RefCell::new(None));
    let body = AgeBody {
        case,
        budget,
        timer,
        observed: observed.clone(),
        seen: seen.clone(),
    };
    let expired = block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        let registered = slot.register(backend).ok().expect("fresh local slot");
        let end = radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, body)
            })
            .await;
        let radio::PortEnd::Uninstalled { joined, .. } = end else {
            panic!("clean uninstall")
        };
        let stopped = slot.retire(joined).ok().expect("the joined slot retires");
        stopped.expired_msdus()
    }));
    let mut trace = seen.borrow_mut().take().expect("the terminal was observed");
    trace.expired_msdus = expired;
    trace
}

#[test]
fn the_s31_backend_passes_the_shared_single_mpdu_admission_age_cases() {
    for case in CASES {
        assert_eq!(
            verify(case, exercise(case)).map_err(|why| (case, why)),
            Ok(())
        );
    }
}
