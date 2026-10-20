//! Actual FIFO context returns across running, queued and posted disable.
use super::*;
use oer_ieee80211_port::conformance::interface_commands::{Case, Drain, Identity, Trace};

struct Witness {
    allocation: alloc::boxed::Box<u8>,
    drops: Rc<Cell<usize>>,
}
impl Witness {
    fn identity(&self, role: VifRole) -> Identity {
        Identity {
            role,
            context: self.allocation.as_ref() as *const u8 as usize,
        }
    }
}
impl Drop for Witness {
    fn drop(&mut self) {
        self.drops.set(self.drops.get().saturating_add(1));
    }
}
struct Contexts;
impl ModelContext for Contexts {
    type Body = ModelBody;
    type Context<'id> = ();
    type StationConfigureContext<'id> = Witness;
    type AccessPointConfigureContext<'id> = Witness;
}
type Backend = ModelBackend<ModelCaps, Contexts>;
type Captured = Trace<Vec<Identity>>;

/// Exercise both interfaces, returning and reusing the original permissions.
pub async fn configure(
    case: Case,
    channel: Channel,
    clock: Rc<dyn Clock>,
) -> Result<Captured, &'static str> {
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("AP")?;
    let (backend, _) = Backend::new(channel, [station, ap], clock);
    match oer_radio_port::run_port(backend, Exercise(case)).await {
        PortEnd::Uninstalled { output, .. } => output,
        _ => Err("configuration failed to uninstall"),
    }
}
fn record<'id, R: InterfaceRole, P: oer_radio_port::PoolPath>(
    permission: ConfigurePermission<'id, R, P, Witness>,
    returned: &mut Vec<Identity>,
) -> ConfigurePermission<'id, R, P, Witness> {
    let (permission, witness) = permission.into_parts();
    returned.push(witness.identity(R::ROLE));
    permission.with_context(witness)
}
struct Exercise(Case);
impl PortBody<Backend> for Exercise {
    type Output = Result<Captured, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, Backend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => {
                return Ended::fail_stopped(failed.fail_stop(), Err("configuration enable"));
            }
        };
        let drops = Rc::new(Cell::new(0));
        let station = Witness {
            allocation: alloc::boxed::Box::new(1),
            drops: drops.clone(),
        };
        let ap = Witness {
            allocation: alloc::boxed::Box::new(2),
            drops: drops.clone(),
        };
        let mut admitted = [
            station.identity(VifRole::Station),
            ap.identity(VifRole::AccessPoint),
        ];
        if self.0.ap_first {
            admitted.swap(0, 1);
        }
        let mut station = resources.station_commands.configure.with_context(station);
        let mut ap = resources.access_point_commands.configure.with_context(ap);
        let mut returned = Vec::new();
        for round in 0..2 {
            let Some(filter) = SupportedFilter::new(ReceiveFilter::NONE) else {
                return stop(owner, Err("configuration filter")).await;
            };
            if self.0.ap_first {
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
            let mut invalid = false;
            let mut consume = |terminal| {
                match terminal {
                    LowerMacTerminal::Configured(Configured::Station(p)) => {
                        assert!(
                            got_station.replace(record(p, &mut returned)).is_none(),
                            "one station permission"
                        );
                    }
                    LowerMacTerminal::Configured(Configured::AccessPoint(p)) => {
                        assert!(
                            got_ap.replace(record(p, &mut returned)).is_none(),
                            "one AP permission"
                        );
                    }
                    _ => invalid = true,
                }
                core::ops::ControlFlow::Continue(())
            };
            if round == 0 && self.0.drain != Drain::Running {
                if self.0.drain == Drain::Posted {
                    for _ in 0..4 {
                        yield_once().await;
                    }
                }
                let disabled = match owner.disable(&mut consume).await {
                    Ok(owner) => owner,
                    Err(failed) => {
                        return Ended::fail_stopped(
                            failed.fail_stop(),
                            Err("configuration disable"),
                        );
                    }
                };
                owner = match disabled.enable().await {
                    Ok(owner) => owner,
                    Err(failed) => {
                        return Ended::fail_stopped(
                            failed.fail_stop(),
                            Err("configuration reenable"),
                        );
                    }
                };
            } else {
                for _ in 0..2 {
                    let event = owner.phase().next_event().await;
                    let Ok(Ok(LowerMacEvent::Terminal(terminal))) = event else {
                        return stop(owner, Err("configuration terminal")).await;
                    };
                    let _flow = consume(terminal);
                }
            }
            if invalid {
                return stop(owner, Err("unexpected configuration terminal")).await;
            }
            let (Some(returned_station), Some(returned_ap)) = (got_station, got_ap) else {
                return stop(owner, Err("configuration permission missing")).await;
            };
            station = returned_station;
            ap = returned_ap;
        }
        let extra_terminal = core::pin::pin!(owner.phase().next_event())
            .poll(&mut core::task::Context::from_waker(
                core::task::Waker::noop(),
            ))
            .is_ready();
        let trace = Trace {
            admitted,
            returned,
            premature_drops: drops.get(),
            extra_terminal,
        };
        drop((station, ap));
        assert_eq!(drops.get(), 2, "both opaque allocations drop after capture");
        stop(owner, Ok(trace)).await
    }
}
pub(super) async fn yield_once() {
    let mut yielded = false;
    core::future::poll_fn(|cx| {
        if yielded {
            core::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            core::task::Poll::Pending
        }
    })
    .await;
}

async fn stop<'p, 'id>(
    owner: Owner<'p, 'id, Backend, oer_radio_port::Enabled>,
    result: Result<Captured, &'static str>,
) -> Ended<'id, Backend, Result<Captured, &'static str>> {
    match owner
        .disable(&mut |_| core::ops::ControlFlow::Continue(()))
        .await
    {
        Ok(owner) => Ended::uninstall(owner, (), result),
        Err(failed) => Ended::fail_stopped(failed.fail_stop(), Err("configuration final disable")),
    }
}
