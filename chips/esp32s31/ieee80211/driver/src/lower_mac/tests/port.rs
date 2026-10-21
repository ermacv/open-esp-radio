use super::{ACCESS_POINT, BSSID, Hardware, Power, RadioNine, STATION, channel, entropy, slot};
use crate::port::*;
use core::{
    convert::Infallible,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee80211_port::*;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    frame::FrameControl,
    phy::{LegacyRate, PhyRate},
};
use oer_radio_port::{self as radio, PortBackend, PortBody, PortPhase};
use oer_time::{Clock, Instant, Timer};
use std::{
    boxed::Box,
    cell::{Cell, RefCell},
    rc::Rc,
    vec::Vec,
};

#[derive(Clone)]
struct TestTimer(Rc<oer_time_virtual::VirtualClock>);
impl Clock for TestTimer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl Timer for TestTimer {
    async fn wait_until(&self, deadline: Instant) {
        self.0.wait_until(deadline).await
    }
}

#[derive(Default)]
struct Observed {
    publications: usize,
    statuses: Vec<(u8, u8)>,
    starts: usize,
    start_channels: Vec<oer_ieee80211_wire::channel::WifiChannel>,
    stops: usize,
    abandoned: bool,
    detached: bool,
    hold_start: bool,
    hold_completion: bool,
    hold_cut: bool,
    hold_stop: bool,
    cut_calls: usize,
    resumed_cuts: usize,
    backoffs: Vec<u16>,
    received: std::collections::VecDeque<TestReceive>,
}
struct Payload(Rc<Cell<usize>>);
impl TxBody for Payload {
    fn bytes(&self) -> &[u8] {
        b"payload"
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct TestDevice {
    hardware: Hardware,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    completed: usize,
    stopped: bool,
}
struct TestReceive {
    bytes: [u8; 26],
    released: Rc<Cell<usize>>,
}
impl ReceiveUnit for TestReceive {
    fn normalized(
        &self,
    ) -> Result<
        oer_esp32s31_ieee80211_mac::rx::NormalizedRxFrame<'_>,
        oer_esp32s31_ieee80211_mac::rx::RxError,
    > {
        Ok(super::normalized(&self.bytes))
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for TestReceive {
    fn drop(&mut self) {
        self.released.set(self.released.get() + 1);
    }
}
impl Device<512> for TestDevice {
    type Profile = oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile;
    type Power = Power;
    type Entropy = fn() -> u32;
    type Timer = TestTimer;
    type Hardware = Hardware;
    type Receive = TestReceive;
    type Body = Payload;
    type Context<'id> = usize;
    type ConfigureContext<'id> = usize;
    fn hardware(&mut self) -> &mut Hardware {
        &mut self.hardware
    }
    fn timer(&self) -> &TestTimer {
        &self.timer
    }
    fn next_deadline(&self) -> Option<Instant> {
        None
    }
    fn take_power_profile(&mut self) -> Option<Self::Power> {
        None
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, radio::ClockError> {
        Ok(Ieee80211Instant::from_micros(
            self.timer.now().as_micros() + 1234,
        ))
    }
    fn poll_start(
        &mut self,
        _: &mut Context<'_>,
        channel: oer_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        if self.observed.borrow().hold_start {
            return Poll::Pending;
        }
        self.observed.borrow_mut().starts += 1;
        self.observed.borrow_mut().start_channels.push(channel);
        self.stopped = false;
        Poll::Ready(Ok(()))
    }
    fn poll_stop(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        if self.observed.borrow().hold_stop {
            return Poll::Pending;
        }
        if !self.stopped {
            self.observed.borrow_mut().stops += 1;
            self.stopped = true;
        }
        Poll::Ready(Ok(()))
    }
    fn poll_receive(
        &mut self,
        receive_room: usize,
        _: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<TestReceive>, Fault>> {
        if receive_room != 0 {
            if let Some(frame) = self.observed.borrow_mut().received.pop_front() {
                return Poll::Ready(Ok(DeviceEvent::Receive(frame)));
            }
        }
        let publications = self.hardware.publications();
        self.observed.borrow_mut().backoffs = self
            .hardware
            .legacy
            .iter()
            .map(|(_, program)| program.contention_window())
            .collect();
        self.observed.borrow_mut().publications = publications;
        if self.completed < publications && !self.observed.borrow().hold_completion {
            let (status, detail) = self
                .observed
                .borrow()
                .statuses
                .get(self.completed)
                .copied()
                .unwrap_or((0, 0));
            let queue = self
                .hardware
                .legacy
                .get(self.completed)
                .map(|p| p.0)
                .unwrap_or(2);
            self.hardware.completion[usize::from(queue)] = Some(
                oer_esp32s31_hal::types::MacTxCompletionObservation::new_model(status, detail),
            );
            self.completed += 1;
            Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE,
            })))
        } else {
            Poll::Pending
        }
    }
    fn poll_rx_cut(
        &mut self,
        _: usize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<TestReceive>, Fault>> {
        self.observed.borrow_mut().cut_calls += 1;
        if self.observed.borrow().hold_cut {
            return Poll::Pending;
        }
        Poll::Ready(Ok(None))
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        self.observed.borrow_mut().resumed_cuts += 1;
        Ok(())
    }
    fn recycle(&mut self, frame: TestReceive) {
        drop(frame);
    }
    fn abandon(&mut self) -> bool {
        self.observed.borrow_mut().abandoned = true;
        false
    }
    fn detach(&mut self) {
        self.observed.borrow_mut().detached = true;
    }
}
type Backend = Esp32s31Backend<TestDevice, 512>;
type Caps = Esp32s31Caps<512>;
type Family = Types<TestDevice, 512>;
type Res<'id> = <Backend as PortBackend>::Resources<'id>;

mod barriers;

fn installed(
    statuses: Vec<(u8, u8)>,
) -> (radio::Installed<Backend>, Rc<RefCell<Observed>>, TestTimer) {
    let timer = TestTimer(Rc::new(oer_time_virtual::VirtualClock::new()));
    let observed = Rc::new(RefCell::new(Observed {
        statuses,
        ..Observed::default()
    }));
    let device = TestDevice {
        hardware: Hardware::default(),
        timer: timer.clone(),
        observed: observed.clone(),
        completed: 0,
        stopped: false,
    };
    let storage = Box::leak(Box::new(PortStorage::new(device)));
    let transmitter = crate::ordinary_tx::OrdinaryTxOwner::new(crate::tx::WifiTxResources {
        slot: slot(),
        policy: oer_esp32s31_ieee80211_mac::tx::runtime::WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: entropy as fn() -> u32,
        timer: timer.clone(),
    })
    .into_idle()
    .ok()
    .expect("idle transmitter");
    let parts = PortParts {
        storage,
        transmitter,
        buffers: core::array::from_fn(|_| slot()),
        config: crate::lower_mac::LowerMacConfig {
            station_address: STATION,
            channel: channel(6),
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: 17,
        },
        access_point_address: IndividualAddress::new(ACCESS_POINT).expect("AP address"),
        coex: &RadioNine,
    };
    (
        Backend::new(parts).ok().expect("valid construction"),
        observed,
        timer,
    )
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..1000 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("port made no bounded progress")
}

struct Exchange {
    transmissions: u8,
    expected: usize,
    close: bool,
}
impl PortBody<Backend> for Exchange {
    type Output = Vec<SingleOutcome<Ack, Infallible>>;
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        let mut resources = resources;
        let mut outcomes = Vec::new();
        {
            let mut phase = owner.phase();
            let permission = resources.station_commands.configure.with_context(42);
            phase.station().configure(
                permission,
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                    Configured::Station(permission),
                )))) => assert_eq!(permission.into_parts().1, 42),
                _ => panic!("configuration terminal"),
            }
            let rate = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                .expect("supported rate");
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            let header = HeaderSpec::new(
                FrameControl::new(oer_ieee80211_wire::frame::DATA_TO_DS).expect("frame control"),
                0,
                IndividualAddress::new(BSSID).expect("receiver"),
                STATION,
                BSSID,
                0x1230,
                None,
                None,
                None,
            )
            .expect("header");
            let frame = ClassFrame::new(TxFrame::Individual {
                frame: SizedFrame::new(
                    AckHeader::new(header).expect("ACK header"),
                    MpduLength::new(64).expect("length"),
                )
                .expect("sized frame"),
                plan: RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::new(self.transmissions)
                        .expect("retry count"),
                    protection: Protection::None,
                }),
            })
            .ok()
            .expect("class length");
            let source = TxSource::Fresh {
                buffer,
                frame,
                body: None,
                protection: TxProtection::Open,
            }
            .check()
            .ok()
            .expect("checked source");
            let prepared =
                <Family as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                    .ok()
                    .expect("prepared source");
            if self.close {
                phase.close_gate(resources.gate);
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) => {
                        resources.gate = phase.open_gate(gate);
                    }
                    _ => panic!("gate terminal"),
                }
            }
            phase
                .station()
                .submit(
                    Attempt {
                        context: 71,
                        ticket,
                        source: prepared,
                        access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                        coex_operation: TxOperation::Data,
                    }
                    .admit(),
                )
                .expect("submit");
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => {
                    let returned = completed.into_parts(&mut resources.terminal_access);
                    assert_eq!(returned.context, 71);
                    assert_eq!(returned.ticket.index(), 0);
                    let (buffer, body) = returned.payload.into_parts();
                    assert!(body.is_none());
                    let AnyBuffer::Sent(sent) = buffer else {
                        panic!("published MPDU returns Sent")
                    };
                    let (mut buffer, body) =
                        <Family as TxBufferPool<'id, BufferClass<512>>>::renew(sent);
                    assert!(body.is_none());
                    assert_eq!(&buffer.bytes_mut()[22..24], &0x1230_u16.to_le_bytes());
                    outcomes.push(returned.outcome);
                }
                _ => panic!("TX terminal"),
            }
        }
        assert_eq!(outcomes.len(), self.expected);
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), outcomes)
    }
}

#[test]
fn port_returns_actual_operation_after_ack_and_gate_cycle() {
    let (backend, observed, _) = installed(Vec::new());
    let result = block_on(radio::run_port(
        backend,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: true,
        },
        oer_radio_port::LocalExecution,
    ));
    let radio::PortEnd::Uninstalled { output, .. } = result else {
        panic!("clean uninstall")
    };
    assert!(output[0].is_delivered());
    let observed = observed.borrow();
    assert_eq!(observed.publications, 1);
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.stops, 1);
    assert!(observed.detached);
    assert!(!observed.abandoned);
}

#[test]
fn port_exhausts_supplied_retry_plan_with_one_terminal() {
    let (backend, observed, _) = installed(std::vec![(5, 0); 3]);
    let result = block_on(radio::run_port(
        backend,
        Exchange {
            transmissions: 3,
            expected: 1,
            close: false,
        },
        oer_radio_port::LocalExecution,
    ));
    let radio::PortEnd::Uninstalled { output, .. } = result else {
        panic!("clean uninstall")
    };
    assert!(
        matches!(output[0], TxOutcome::Ended(SingleEnd::AckTimeout(air)) if air.transmissions.get() == 3)
    );
    assert_eq!(observed.borrow().publications, 3);
    // Standard BE CWmin=15 doubles after each unanswered publication;
    // the fixed entropy selects 8, 24 and 56 slots from those windows.
    assert_eq!(observed.borrow().backoffs, [8, 24, 56]);
}

fn submit_mpdu<'k, 'p, 'id: 'p + 'k>(
    phase: &mut Esp32s31Phase<'p, 'id, TestDevice, 512>,
    buffer: Buffer<'id, 512>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<oer_radio_port::LocalId<'id>>, 2>>,
    body: Option<Payload>,
    key: Option<&'k PairwiseKey<'id, Station>>,
) {
    submit_mpdu_budget(phase, buffer, ticket, body, key, 1);
}
fn submit_mpdu_budget<'k, 'p, 'id: 'p + 'k>(
    phase: &mut Esp32s31Phase<'p, 'id, TestDevice, 512>,
    buffer: Buffer<'id, 512>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<oer_radio_port::LocalId<'id>>, 2>>,
    body: Option<Payload>,
    key: Option<&'k PairwiseKey<'id, Station>>,
    budget: u8,
) {
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
    let frame = ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(
            AckHeader::new(header).expect("ACK"),
            MpduLength::new(64).expect("length"),
        )
        .expect("frame"),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(budget).expect("nonzero test budget"),
            protection: Protection::None,
        }),
    })
    .ok()
    .expect("class");
    let source = TxSource::Fresh {
        buffer,
        frame,
        body,
        protection: match key {
            Some(key) => TxProtection::Ccmp(NumberedKey {
                key,
                packet_number: oer_ieee80211_wire::ccmp::CcmpPacketNumber::new(7).expect("PN"),
            }),
            None => TxProtection::Open,
        },
    }
    .check()
    .ok()
    .expect("layout");
    let source = <Family as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
        .ok()
        .expect("prepare");
    phase
        .station()
        .submit(
            Attempt {
                context: 91,
                ticket,
                source,
                access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        )
        .expect("submit");
}

#[derive(Clone, Copy, Debug)]
enum Suspension {
    Enable,
    Publication,
    Group,
    Disable,
}
struct SuspendBody {
    at: Suspension,
    dropped: Rc<Cell<usize>>,
}
impl PortBody<Backend> for SuspendBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        match self.at {
            Suspension::Publication => {
                let mut phase = owner.phase();
                let [buffer, ..] = resources.buffers;
                let [ticket, ..] = resources.tickets;
                submit_mpdu(
                    &mut phase,
                    buffer,
                    ticket,
                    Some(Payload(self.dropped)),
                    None,
                );
                let _ = phase.next_event().await;
            }
            Suspension::Group => {
                let mut phase = owner.phase();
                let (start, bank) = resources.station_association.into_parts();
                let (_, [slot]) = bank.into_parts();
                let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
                phase.station().install_group_key(
                    &association,
                    resources.station_group_permission,
                    slot,
                    oer_ieee80211_wire::ccmp::GroupKeyId::new(1).expect("KeyID"),
                    oer_ieee80211_wire::ccmp::Ccmp128Key::new(&[3; 16]),
                );
                let _ = phase.next_event().await;
            }
            _ => {}
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn cancelling_each_device_wait_quarantines_without_dropping_an_admitted_body() {
    for at in [
        Suspension::Enable,
        Suspension::Publication,
        Suspension::Group,
        Suspension::Disable,
    ] {
        let (backend, observed, _) = installed(Vec::new());
        {
            let mut observed = observed.borrow_mut();
            match at {
                Suspension::Enable => observed.hold_start = true,
                Suspension::Publication => observed.hold_completion = true,
                Suspension::Group => observed.hold_cut = true,
                Suspension::Disable => observed.hold_stop = true,
            }
        }
        let dropped = Rc::new(Cell::new(0));
        let mut future = Box::pin(radio::run_port(
            backend,
            SuspendBody {
                at,
                dropped: dropped.clone(),
            },
            oer_radio_port::LocalExecution,
        ));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..12 {
            assert!(future.as_mut().poll(&mut cx).is_pending(), "wait {at:?}");
        }
        if matches!(at, Suspension::Publication) {
            assert_eq!(observed.borrow().publications, 1);
        }
        if matches!(at, Suspension::Group) {
            assert!(observed.borrow().cut_calls > 0);
        }
        drop(future);
        assert!(observed.borrow().abandoned, "wait {at:?}");
        assert_eq!(dropped.get(), 0);
        assert!(!observed.borrow().detached);
    }
}

struct GroupCycle;
impl PortBody<Backend> for GroupCycle {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
            let id = |id| oer_ieee80211_wire::ccmp::GroupKeyId::new(id).expect("KeyID");
            phase.station().install_group_key(
                &association,
                resources.station_group_permission,
                slot,
                id(1),
                oer_ieee80211_wire::ccmp::Ccmp128Key::new(&[1; 16]),
            );
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::Station { key, permission },
                )))) => (key, permission),
                _ => panic!("installed"),
            };
            phase.station().replace_group_key(
                permission,
                key,
                id(2),
                oer_ieee80211_wire::ccmp::Ccmp128Key::new(&[2; 16]),
            );
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyReplaced {
                    key,
                    permission,
                }))) => (key, permission),
                _ => panic!("replaced"),
            };
            phase.station().select_group_key(permission, key);
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected {
                    key,
                    permission,
                }))) => (key, permission),
                _ => panic!("selected"),
            };
            phase.station().remove_group_key(permission, key);
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                    Removed::Station(RemovedKey::Group { .. })
                ))))
            ));
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn every_group_mutation_has_a_finite_rx_cut_and_returns_its_permission_once() {
    let (backend, observed, _) = installed(Vec::new());
    assert!(matches!(
        block_on(radio::run_port(
            backend,
            GroupCycle,
            oer_radio_port::LocalExecution
        )),
        radio::PortEnd::Uninstalled { .. }
    ));
    assert_eq!(observed.borrow().resumed_cuts, 4);
}

struct Expire {
    clock: TestTimer,
}
impl PortBody<Backend> for Expire {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        mut resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            phase.close_gate(resources.gate);
            let gate = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) => gate,
                _ => panic!("gate"),
            };
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            submit_mpdu(&mut phase, buffer, ticket, None, None);
            self.clock
                .0
                .advance(EdcaParameterSet::DEFAULT_MSDU_LIFETIME)
                .expect("advance clock");
            let completed = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => completed,
                _ => panic!("expiry"),
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            assert!(matches!(returned.outcome, TxOutcome::Expired { msdus: 1 }));
            let (memory, _) = returned.payload.into_parts();
            assert!(matches!(memory, AnyBuffer::Fresh(_)));
            resources.gate = phase.open_gate(gate);
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn queued_msdu_expires_from_submit_without_publication_behind_a_closed_gate() {
    let (backend, observed, clock) = installed(Vec::new());
    assert!(matches!(
        block_on(radio::run_port(
            backend,
            Expire { clock },
            oer_radio_port::LocalExecution
        )),
        radio::PortEnd::Uninstalled { .. }
    ));
    assert_eq!(observed.borrow().publications, 0);
}

struct ReceiveBody {
    observed: Rc<RefCell<Observed>>,
    released: Rc<Cell<usize>>,
    lose: bool,
}
impl PortBody<Backend> for ReceiveBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources.station_commands.configure.with_context(7),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            self.observed.borrow_mut().received.push_back(TestReceive {
                bytes: super::data_frame(STATION),
                released: self.released.clone(),
            });
            let frame = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, meta })) => {
                    assert_eq!(meta.common().rate, RxEvidence::Unavailable);
                    assert_eq!(meta.common().rssi_dbm, RxEvidence::HardwareObserved(-42));
                    frame
                }
                _ => panic!("received MPDU"),
            };
            assert_eq!(frame.bytes(), &super::data_frame(STATION));
            assert_eq!(self.released.get(), 0);
            if self.lose {
                drop(frame);
                assert!(matches!(
                    phase.next_event().await,
                    Err(radio::EventEnd::Quarantined { .. })
                ));
            } else {
                phase.recycle(frame);
            }
        }
        if self.lose {
            return radio::Ended::fail_stopped(owner.fail_stop().await, ());
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        assert_eq!(self.released.get(), 1);
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn received_bss_mpdu_keeps_its_original_owner_until_explicit_recycling() {
    let (backend, observed, _) = installed(Vec::new());
    let released = Rc::new(Cell::new(0));
    assert!(matches!(
        block_on(radio::run_port(
            backend,
            ReceiveBody {
                observed,
                released: released.clone(),
                lose: false
            },
            oer_radio_port::LocalExecution
        )),
        radio::PortEnd::Uninstalled { .. }
    ));
    assert_eq!(released.get(), 1);
}
#[test]
fn dropping_a_received_owner_quarantines_its_allocation_and_poison_is_permanent() {
    let (backend, observed, _) = installed(Vec::new());
    let released = Rc::new(Cell::new(0));
    let result = block_on(radio::run_port(
        backend,
        ReceiveBody {
            observed: observed.clone(),
            released: released.clone(),
            lose: true,
        },
        oer_radio_port::LocalExecution,
    ));
    assert!(matches!(result, radio::PortEnd::FailStopped { .. }));
    drop(result);
    assert_eq!(released.get(), 0);
    assert!(observed.borrow().abandoned);
}

struct Protected {
    dropped: Rc<Cell<usize>>,
}
impl PortBody<Backend> for Protected {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        mut resources: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let ([slot], _) = bank.into_parts();
            let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
            phase.station().configure(
                resources.station_commands.configure.with_context(19),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            let key = phase.station().install_pairwise_key(
                &association,
                slot,
                IndividualAddress::new(BSSID).expect("peer"),
                oer_ieee80211_wire::ccmp::Ccmp128Key::new(&[9; 16]),
            );
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            submit_mpdu(
                &mut phase,
                buffer,
                ticket,
                Some(Payload(self.dropped.clone())),
                Some(&key),
            );
            let completed = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => completed,
                _ => panic!("protected TX"),
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            assert!(returned.outcome.is_delivered());
            assert_eq!(self.dropped.get(), 0);
            let (buffer, body) = returned.payload.into_parts();
            assert!(body.is_none());
            let AnyBuffer::Sent(mpdu) = buffer else {
                panic!("Sent")
            };
            assert_eq!(mpdu.packet_number().expect("caller PN").value(), 7);
            assert_eq!(mpdu.body().expect("retained body").bytes(), b"payload");
            phase.station().remove_pairwise_key(association, key);
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                    Removed::Station(RemovedKey::Pairwise { .. })
                ))))
            ));
            let rate = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                .expect("rate");
            let source = TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::MIN,
                    protection: Protection::None,
                })),
            }
            .check()
            .ok()
            .expect("layout");
            let refused =
                <Family as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                    .err()
                    .expect("retired key refused before admission");
            assert_eq!(refused.reason, PreparationError::RetransmissionKey);
            drop(refused);
            assert_eq!(self.dropped.get(), 1);
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn caller_pn_and_external_body_survive_tx_and_a_removed_key_refuses_retransmission() {
    let (backend, observed, _) = installed(Vec::new());
    let dropped = Rc::new(Cell::new(0));
    assert!(matches!(
        block_on(radio::run_port(
            backend,
            Protected {
                dropped: dropped.clone()
            },
            oer_radio_port::LocalExecution
        )),
        radio::PortEnd::Uninstalled { .. }
    ));
    assert_eq!(dropped.get(), 1);
    assert_eq!(observed.borrow().publications, 1);
}

#[test]
fn emergency_erasure_reaches_the_last_ap_peer_as_well_as_station_and_group_keys() {
    use oer_esp32s31_ieee80211_mac::crypto::*;
    let mut hardware = Hardware::default();
    let station = install_sta_pairwise_ccmp(&mut hardware, BSSID, &[1; 16]).expect("STA PTK");
    let group = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_wire::ccmp::GroupKeyId::new(0).expect("KeyID"),
        &[2; 16],
    )
    .expect("GTK");
    let ap = install_ap_pairwise_ccmp(
        &mut hardware,
        super::PEER,
        u16::from(AP_PAIRWISE_SLOT_COUNT),
        &[3; 16],
    )
    .expect("last AP AID");
    let ap_group = install_ap_group_ccmp(
        &mut hardware,
        ApGroupKeyId::new(1).expect("AP KeyID"),
        &[4; 16],
    )
    .expect("AP GTK");
    assert_eq!(hardware.installed_keys.len(), 4);
    erase_lower_mac_keys(&mut hardware);
    assert!(hardware.installed_keys.is_empty());
    for index in [
        station.hardware_index(),
        group.hardware_index(),
        ap.hardware_index(),
        ap_group.hardware_index(),
    ] {
        assert!(hardware.cleared_keys.contains(&index));
    }
}

#[test]
fn clean_uninstall_returns_all_dma_and_core_owners_for_a_fresh_installation() {
    let (backend, observed, _) = installed(Vec::new());
    let first = block_on(radio::run_port(
        backend,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: true,
        },
        oer_radio_port::LocalExecution,
    ));
    let radio::PortEnd::Uninstalled {
        uninstalled: Uninstalled::Stopped(stopped),
        ..
    } = first
    else {
        panic!("stopped owners")
    };
    let second = stopped
        .reinstall(18)
        .ok()
        .expect("all buffers and core returned");
    let second = block_on(radio::run_port(
        second,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: false,
        },
        oer_radio_port::LocalExecution,
    ));
    assert!(matches!(
        second,
        radio::PortEnd::Uninstalled {
            uninstalled: Uninstalled::Stopped(_),
            ..
        }
    ));
    assert_eq!(observed.borrow().publications, 2);
    assert_eq!(observed.borrow().starts, 2);
    assert_eq!(observed.borrow().stops, 2);
}

struct Tune(Channel);
impl PortBody<Backend> for Tune {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        mut owner: radio::Owner<'p, 'id, Backend, radio::Disabled>,
        _: Res<'id>,
    ) -> radio::Ended<'id, Backend, ()> {
        owner.setup().set_channel(
            SupportedChannel::new(
                oer_radio_regulatory::RegulatoryDomain::of(
                    oer_radio_regulatory::Country::new(*b"DE").expect("test country"),
                )
                .expect("domain fixture")
                .permit(self.0)
                .expect("permitted test channel"),
            )
            .expect("supported test channel"),
        );
        let owner = owner.enable().await.ok().expect("enable");
        let owner = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("disable");
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn frontend_channel_setting_survives_normal_stop_and_reinstall() {
    let (backend, observed, _) = installed(Vec::new());
    let tuned = Channel::from_wifi_channel(channel(11));
    let first = block_on(radio::run_port(backend, Tune(tuned), radio::LocalExecution));
    let radio::PortEnd::Uninstalled {
        uninstalled: Uninstalled::Stopped(stopped),
        ..
    } = first
    else {
        panic!("the tuned installation stops normally");
    };
    let installed = stopped
        .reinstall(18)
        .ok()
        .expect("the stopped owners reinstall");
    let _ = block_on(radio::run_port(
        installed,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: false,
        },
        radio::LocalExecution,
    ));
    assert_eq!(observed.borrow().start_channels, [channel(11), channel(11)]);
}
