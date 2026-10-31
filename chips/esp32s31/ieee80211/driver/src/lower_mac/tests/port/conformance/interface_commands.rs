//! The shared configuration-command suite against the S31 backend: both
//! interfaces' permissions return in FIFO order with the contexts they were
//! lent, through the running stream or through a disable drain, and are
//! submitted again.
use super::*;
use oer_ieee80211_testing_conformance::interface_commands::{
    Case, Drain, Harness, Identity, Trace, check,
};

struct Commands;

struct CommandBody {
    case: Case,
    drops: Rc<Cell<usize>>,
    /// The trace as observed: a fail-stop may cancel the body first.
    seen: Rc<RefCell<Option<Trace<Vec<Identity>>>>>,
}

fn identity(role: VifRole, tag: &Tag) -> Identity {
    Identity {
        role,
        context: tag.0,
    }
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for CommandBody {
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
        let witness = |value| Tag(value, Some(self.drops.clone()));
        let mut station = resources
            .station_commands
            .configure
            .with_context(witness(1));
        let mut ap = resources
            .access_point_commands
            .configure
            .with_context(witness(2));
        let mut admitted = [
            Identity {
                role: VifRole::Station,
                context: 1,
            },
            Identity {
                role: VifRole::AccessPoint,
                context: 2,
            },
        ];
        if self.case.ap_first {
            admitted.swap(0, 1);
        }
        let mut returned = Vec::new();
        for round in 0..2 {
            let filter = SupportedFilter::new(ReceiveFilter::NONE).expect("filter");
            if self.case.ap_first {
                owner.phase().access_point().configure(ap, filter);
                owner
                    .phase()
                    .station()
                    .configure(station, StationReceive::Off);
            } else {
                owner
                    .phase()
                    .station()
                    .configure(station, StationReceive::Off);
                owner.phase().access_point().configure(ap, filter);
            }
            let mut got_station = None;
            let mut got_ap = None;
            let mut consume = |terminal| {
                match terminal {
                    LowerMacTerminal::Configured(Configured::Station(permission)) => {
                        let (permission, tag) = permission.into_parts();
                        returned.push(identity(VifRole::Station, &tag));
                        assert!(
                            got_station.replace(permission.with_context(tag)).is_none(),
                            "one station permission"
                        );
                    }
                    LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                        let (permission, tag) = permission.into_parts();
                        returned.push(identity(VifRole::AccessPoint, &tag));
                        assert!(
                            got_ap.replace(permission.with_context(tag)).is_none(),
                            "one AP permission"
                        );
                    }
                    _ => panic!("an unexpected configuration terminal"),
                }
                core::ops::ControlFlow::Continue(())
            };
            if round == 0 && self.case.drain != Drain::Running {
                if self.case.drain == Drain::Posted {
                    for _ in 0..4 {
                        barriers::yield_once().await;
                    }
                }
                let Ok(disabled) = owner.disable(&mut consume).await else {
                    panic!("disable drains the commands")
                };
                let Ok(enabled) = disabled.enable().await else {
                    panic!("enable again")
                };
                owner = enabled;
            } else {
                for _ in 0..2 {
                    let Ok(Ok(LowerMacEvent::Terminal(terminal))) =
                        owner.phase().next_event().await
                    else {
                        panic!("a configuration terminal")
                    };
                    let _flow = consume(terminal);
                }
            }
            station = got_station.expect("the station permission returned");
            ap = got_ap.expect("the AP permission returned");
        }
        let extra_terminal = pin!(owner.phase().next_event())
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready();
        let premature_drops = self.drops.get();
        drop((station, ap));
        assert_eq!(
            self.drops.get(),
            2,
            "both contexts drop with their permissions"
        );
        *self.seen.borrow_mut() = Some(Trace {
            admitted,
            returned,
            premature_drops,
            extra_terminal,
        });
        let Ok(owner) = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

impl Harness for Commands {
    type Returns = Vec<Identity>;
    async fn exercise(&mut self, case: Case) -> Trace<Vec<Identity>> {
        let (parts, _, _) = installed(Vec::new());
        let seen = Rc::new(RefCell::new(None));
        let body = CommandBody {
            case,
            drops: Rc::new(Cell::new(0)),
            seen: seen.clone(),
        };
        let Report::Uninstalled { .. } = run_body(parts, body).await else {
            panic!("clean uninstall")
        };
        seen.borrow_mut().take().expect("both rounds were observed")
    }
}

#[test]
fn the_s31_backend_passes_the_shared_interface_command_suite() {
    let report = block_on(check(&mut Commands, Environment::Model));
    assert_eq!(
        report.map(|report| (report.passed, report.model_only)),
        Ok((6, 0))
    );
}
