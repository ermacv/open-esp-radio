use super::*;
use crate::lower_mac_owner::conformance::interface_commands::{self, Identity, Trace};
use oer_ieee80211_lower_mac::InterfaceRole;

struct Witness {
    allocation: Box<u8>,
    drops: Rc<Cell<usize>>,
}
impl Witness {
    fn identity(&self, role: VifRole) -> Identity {
        Identity {
            role,
            context: &*self.allocation as *const u8 as usize,
        }
    }
}
impl Drop for Witness {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
struct ConfigurationContexts;
impl ModelContext for ConfigurationContexts {
    type Body = ModelBody;
    type Context<'id> = ();
    type StationConfigureContext<'id> = Witness;
    type AccessPointConfigureContext<'id> = Witness;
}
type ConfigurationBackend = ModelBackend<ModelCaps, ConfigurationContexts>;

#[derive(Clone, Copy)]
enum Drain {
    Running,
    Queued,
    Posted,
}
struct ConfigureAcrossDisable {
    drain: Drain,
    ap_first: bool,
    drops: Rc<Cell<usize>>,
}
fn record<'id, R: InterfaceRole, Path: oer_radio_port::PoolPath>(
    permission: ConfigurePermission<'id, R, Path, Witness>,
    returned: &mut Vec<Identity>,
) -> ConfigurePermission<'id, R, Path, Witness> {
    let (permission, witness) = permission.into_parts();
    returned.push(witness.identity(R::ROLE));
    permission.with_context(witness)
}
impl PortBody<ConfigurationBackend> for ConfigureAcrossDisable {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ConfigurationBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ConfigurationBackend, ()> {
        let mut enabled = owner.enable().await.ok().unwrap();
        let station_context = Witness {
            allocation: Box::new(1),
            drops: self.drops.clone(),
        };
        let ap_context = Witness {
            allocation: Box::new(2),
            drops: self.drops.clone(),
        };
        let mut admitted = [
            station_context.identity(VifRole::Station),
            ap_context.identity(VifRole::AccessPoint),
        ];
        if self.ap_first {
            admitted.swap(0, 1);
        }
        let mut station = resources
            .station_commands
            .configure
            .with_context(station_context);
        let mut access_point = resources
            .access_point_commands
            .configure
            .with_context(ap_context);
        let mut returned = Vec::new();
        for round in 0..2 {
            if self.ap_first {
                enabled.phase().access_point().configure(
                    access_point,
                    SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
                );
                enabled
                    .phase()
                    .station()
                    .configure(station, StationReceive::Off);
            } else {
                enabled
                    .phase()
                    .station()
                    .configure(station, StationReceive::Off);
                enabled.phase().access_point().configure(
                    access_point,
                    SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
                );
            }
            let mut got_station = None;
            let mut got_ap = None;
            let mut consume = |terminal| {
                match terminal {
                    LowerMacTerminal::Configured(Configured::Station(permission)) => {
                        assert!(
                            got_station
                                .replace(record(permission, &mut returned))
                                .is_none()
                        );
                    }
                    LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                        assert!(got_ap.replace(record(permission, &mut returned)).is_none());
                    }
                    _ => panic!("configuration terminals only"),
                }
                core::ops::ControlFlow::Continue(())
            };
            if round == 0 && !matches!(self.drain, Drain::Running) {
                if matches!(self.drain, Drain::Posted) {
                    yield_now().await;
                }
                let disabled = enabled.disable(&mut consume).await.ok().unwrap();
                enabled = disabled.enable().await.ok().unwrap();
            } else {
                for _ in 0..2 {
                    let LowerMacEvent::Terminal(terminal) =
                        enabled.phase().next_event().await.unwrap().unwrap()
                    else {
                        panic!("configuration terminal")
                    };
                    let _flow = consume(terminal);
                }
            }
            station = got_station.unwrap();
            access_point = got_ap.unwrap();
        }
        let extra_terminal = core::pin::pin!(enabled.phase().next_event())
            .poll(&mut core::task::Context::from_waker(Waker::noop()))
            .is_ready();
        assert_eq!(
            interface_commands::verify(Trace {
                admitted,
                returned,
                premature_drops: self.drops.get(),
                extra_terminal
            }),
            Ok(())
        );
        drop((station, access_point));
        assert_eq!(self.drops.get(), 2);
        Ended::uninstall(
            enabled
                .disable(&mut |_| panic!("all permissions returned"))
                .await
                .ok()
                .unwrap(),
            (),
            (),
        )
    }
}
#[test]
fn configure_conformance_preserves_fifo_context_ownership_and_reuse_across_disable() {
    for drain in [Drain::Running, Drain::Queued, Drain::Posted] {
        for ap_first in [false, true] {
            let (backend, _) = ConfigurationBackend::new(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
                [
                    IndividualAddress::new(STATION).unwrap(),
                    IndividualAddress::new(ACCESS_POINT).unwrap(),
                ],
            );
            let _port_end = block_on(run_port(
                backend,
                ConfigureAcrossDisable {
                    drain,
                    ap_first,
                    drops: Rc::new(Cell::new(0)),
                },
            ));
        }
    }
}

struct ReceiveAcrossDisable;
impl PortBody<ModelBackend> for ReceiveAcrossDisable {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!()
        };
        let air = enabled.phase().held.air.clone();
        let meta = ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -95);
        for _ in 0..MODEL_RX_EVENTS + 1 {
            air.receive(&[1], meta);
        }
        for _ in 0..3 {
            yield_now().await;
        }
        let mut frames = Vec::new();
        let event = enabled.phase().next_event().await.unwrap().unwrap();
        let LowerMacEvent::Received { frame, .. } = event else {
            panic!()
        };
        frames.push(frame);
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("RX cannot reach the terminal sink"))
            .await
        else {
            panic!()
        };
        assert_eq!(frames.len(), 1);
        let Ok(mut enabled) = disabled.enable().await else {
            panic!()
        };
        assert_eq!(
            enabled.phase().held.rx_out.get(),
            1,
            "only the buffer already lent to the body stays charged"
        );
        assert!(
            matches!(enabled.phase().next_event().await, Ok(Err(EventsLost))),
            "an unobserved loss survives disable"
        );
        air.receive(&[2], meta);
        let LowerMacEvent::Received { frame, .. } =
            enabled.phase().next_event().await.unwrap().unwrap()
        else {
            panic!("disable recycled unread RX capacity")
        };
        assert_eq!(frame.bytes(), &[2]);
        assert_eq!(enabled.phase().held.rx_out.get(), 2);
        enabled.phase().recycle(frame);
        for _ in 0..3 {
            yield_now().await;
        }
        assert_eq!(
            enabled.phase().held.rx_out.get(),
            1,
            "the pre-disable owner is still charged"
        );
        for frame in frames {
            enabled.phase().recycle(frame);
        }
        for _ in 0..3 {
            yield_now().await;
        }
        assert_eq!(enabled.phase().held.rx_out.get(), 0);
        air.receive(&[3], meta);
        let event = enabled.phase().next_event().await.unwrap().unwrap();
        let LowerMacEvent::Received { frame, .. } = event else {
            panic!()
        };
        assert_eq!(frame.bytes(), [3]);
        enabled.phase().recycle(frame);
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("all events consumed"))
            .await
        else {
            panic!()
        };
        Ended::uninstall(disabled, (), ())
    }
}

#[test]
fn disable_preserves_borrowed_rx_capacity_and_unobserved_loss() {
    let (backend, _) = backend();
    let _port_end = block_on(run_port(backend, ReceiveAcrossDisable));
}

#[derive(Clone, Copy)]
enum Release {
    Ack,
    Retry,
    Watchdog,
}
struct ConfigureBarrier {
    release: Release,
    ap_first: bool,
}
impl Work for ConfigureBarrier {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            mut terminal_access,
            station_commands,
            access_point_commands,
            ..
        } = resources;
        let head = tickets[0].index();
        let tail = tickets[1].index();
        let air = phase.held.air.clone();
        air.hold_completion(WmmAccessCategory::BestEffort, true);
        air.answer(if matches!(self.release, Release::Retry) {
            AirAnswer::NoResponse
        } else {
            AirAnswer::Ack
        });
        submit(phase, tickets.remove(0), small.remove(0), None);
        submit(phase, tickets.remove(0), small.remove(0), None);
        for _ in 0..4 {
            yield_now().await;
        }
        assert_eq!(air.sent().len(), 1);
        if self.ap_first {
            phase.access_point().configure(
                access_point_commands.configure,
                SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
            );
            phase
                .station()
                .configure(station_commands.configure, StationReceive::Off);
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                    Configured::AccessPoint(_)
                ))))
            ));
        } else {
            phase
                .station()
                .configure(station_commands.configure, StationReceive::Off);
            phase.access_point().configure(
                access_point_commands.configure,
                SupportedFilter::new(ReceiveFilter::NONE).unwrap(),
            );
        }
        for _ in 0..4 {
            yield_now().await;
        }
        assert!(
            phase.held.events.is_empty(),
            "the published head and queued successor precede Configured"
        );
        if matches!(self.release, Release::Watchdog) {
            air.set_time(oer_time::Instant::from_micros(
                MODEL_PUBLICATION_BOUND.as_micros(),
            ));
        } else {
            air.hold_completion(WmmAccessCategory::BestEffort, false);
        }
        for expected in [head, tail] {
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Single(completed),
            )))) = phase.next_event().await
            else {
                panic!("TX terminal before configuration")
            };
            let parts = completed.into_parts(&mut terminal_access);
            assert_eq!(parts.ticket.index(), expected, "per-AC completion order");
            if expected == tail || matches!(self.release, Release::Retry) {
                assert_eq!(
                    parts.outcome,
                    TxOutcome::Withdrawn(WithdrawCause::GateClosed)
                );
            } else if matches!(self.release, Release::Watchdog) {
                assert_eq!(
                    parts.outcome,
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
                );
            } else {
                assert!(matches!(
                    parts.outcome,
                    TxOutcome::Ended(SingleEnd::Delivered { .. })
                ));
            }
        }
        assert!(matches!(
            phase.next_event().await,
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                Configured::Station(_)
            ))))
        ));
        if !self.ap_first {
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                    Configured::AccessPoint(_)
                ))))
            ));
        }
        assert_eq!(
            air.sent().len(),
            1,
            "the configuration barrier never publishes the successor or an old retry"
        );
    }
}
#[test]
fn configure_waits_for_own_publication_preserves_ac_order_and_holds_later_fifo_commands() {
    for release in [Release::Ack, Release::Retry, Release::Watchdog] {
        for ap_first in [false, true] {
            let _port_end = block_on(run_port(
                backend().0,
                Body(ConfigureBarrier { release, ap_first }),
            ));
        }
    }
}
