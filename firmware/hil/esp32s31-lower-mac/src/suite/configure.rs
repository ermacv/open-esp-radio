//! Opaque affine context tags and actual FIFO returns, including disable drain.
use super::*;
use core::sync::atomic::Ordering;
use oer_hil_protocol::lower_mac::{Trace, configure_trace};
use oer_ieee80211_port::conformance::interface_commands::{
    self as contract, Case, Drain, Identity,
};

// Same one-byte context storage as the image's original u8 composition.
// Destruction is observed with the existing image ownership counter.
#[repr(u8)]
pub(super) enum Witness {
    Station,
    AccessPoint,
    Discovery,
}
impl Witness {
    fn identity(&self, role: VifRole) -> Identity {
        Identity {
            role,
            context: match self {
                Self::Station => 0,
                Self::AccessPoint => 1,
                Self::Discovery => 2,
            },
        }
    }
}
impl Drop for Witness {
    fn drop(&mut self) {
        if !matches!(self, Self::Discovery) {
            ownership::DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }
}
type Returns = heapless::Vec<Identity, 8>;
fn record<'id, R: InterfaceRole, P: radio::PoolPath>(
    permission: ConfigurePermission<'id, R, P, Witness>,
    returned: &mut Returns,
) -> ConfigurePermission<'id, R, P, Witness> {
    let (permission, witness) = permission.into_parts();
    returned
        .push(witness.identity(R::ROLE))
        .expect("configuration capture capacity");
    permission.with_context(witness)
}
pub(super) async fn run<'p, 'id>(
    mut owner: Owner<'p, 'id, Backend, radio::Enabled>,
    resources: <Backend as PortBackend>::Resources<'id>,
    case: Case,
) -> Ended<'id, Backend, Result<Trace, &'static str>> {
    ownership::DROPS.store(0, Ordering::Relaxed);
    oer_trace::start(port::trace::Publication::CHANNEL.mask());
    let station = Witness::Station;
    let ap = Witness::AccessPoint;
    let mut admitted = [
        station.identity(VifRole::Station),
        ap.identity(VifRole::AccessPoint),
    ];
    if case.ap_first {
        admitted.swap(0, 1);
    }
    let mut station = resources.station_commands.configure.with_context(station);
    let mut ap = resources.access_point_commands.configure.with_context(ap);
    let mut returned = Returns::new();
    for round in 0..2 {
        if case.ap_first {
            owner.phase().access_point().configure(
                ap,
                SupportedFilter::new(ReceiveFilter::NONE).expect("filter"),
            );
            owner
                .phase()
                .station()
                .configure(station, StationReceive::Off);
        } else {
            owner
                .phase()
                .station()
                .configure(station, StationReceive::Off);
            owner.phase().access_point().configure(
                ap,
                SupportedFilter::new(ReceiveFilter::NONE).expect("filter"),
            );
        }
        let mut got_station = None;
        let mut got_ap = None;
        let mut consume = |terminal| {
            match terminal {
                LowerMacTerminal::Configured(Configured::Station(p)) => assert!(
                    got_station.replace(record(p, &mut returned)).is_none(),
                    "one station permission"
                ),
                LowerMacTerminal::Configured(Configured::AccessPoint(p)) => assert!(
                    got_ap.replace(record(p, &mut returned)).is_none(),
                    "one AP permission"
                ),
                _ => panic!("only configuration terminals"),
            }
            core::ops::ControlFlow::Continue(())
        };
        if round == 0 && case.drain != Drain::Running {
            if case.drain == Drain::Posted {
                while TRACE
                    .records()
                    .filter(|row| row.decode::<port::trace::TerminalPosted>().is_some())
                    .count()
                    < 2
                {
                    barrier::progress().await;
                }
            }
            let disabled = match owner.disable(&mut consume).await {
                Ok(owner) => owner,
                Err(failed) => {
                    return Ended::fail_stopped(
                        failed.fail_stop().await,
                        Err("configuration disable"),
                    );
                }
            };
            owner = match disabled.enable().await {
                Ok(owner) => owner,
                Err(failed) => {
                    return Ended::fail_stopped(
                        failed.fail_stop().await,
                        Err("configuration reenable"),
                    );
                }
            };
        } else {
            let mut phase = owner.phase();
            for _ in 0..2 {
                loop {
                    match phase.next_event().await {
                        Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                            let _ = consume(terminal);
                            break;
                        }
                        Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                        Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
                        _ => panic!("configuration event"),
                    }
                }
            }
        }
        station = got_station.expect("station returned");
        ap = got_ap.expect("AP returned");
    }
    let mut phase = owner.phase();
    let extra_terminal = loop {
        let event = {
            let mut next = core::pin::pin!(phase.next_event());
            // Poll once with this task's SRAM-backed Embassy waker. A noop
            // waker can escape through the event source to a strict ISR.
            core::future::poll_fn(|cx| core::task::Poll::Ready(next.as_mut().poll(cx))).await
        };
        match event {
            core::task::Poll::Pending => break false,
            core::task::Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) => {
                phase.recycle(frame)
            }
            core::task::Poll::Ready(Ok(Ok(LowerMacEvent::Tbtt(_)))) => {}
            _ => break true,
        }
    };
    drop(phase);
    let raw = contract::Trace {
        admitted,
        returned,
        premature_drops: ownership::DROPS.load(Ordering::Relaxed) as usize,
        extra_terminal,
    };
    let result = configure_trace(&raw)
        .map(Trace::Configure)
        .ok_or("configuration trace extent");
    drop((station, ap));
    assert_eq!(
        ownership::DROPS.load(Ordering::Relaxed),
        2,
        "opaque contexts drop after capture"
    );
    let owner = match owner
        .disable(&mut |_| panic!("all configuration permissions returned"))
        .await
    {
        Ok(owner) => owner,
        Err(failed) => {
            return Ended::fail_stopped(
                failed.fail_stop().await,
                Err("configuration final disable"),
            );
        }
    };
    Ended::uninstall(owner, (), result)
}
