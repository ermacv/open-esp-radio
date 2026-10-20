//! The cooperative S31 register/RX owner used by the port driver on P2.
use super::*;
use crate::{
    irq::{RadioDoorbell, WifiIrqLine, WifiIrqRoutes},
    runtime::{
        WifiRetuneFailure, WifiRetunePending, WifiRuntimeContext, WifiRuntimeParts, WifiStopped,
    },
    rx::{
        arena::WifiRxParts,
        receive::{CONTINUATION_WAIT, IMMEDIATE_CONTINUATIONS},
        session::ReceiveSession,
        sink::{frame_control_preview, unclassified_preview},
        transaction::{CompletedUnit, Preview, Publisher},
        upper::W1_INGRESS,
    },
    startup::WifiMacPlatform,
};
use oer_esp32s31_hal::{
    ieee80211::dma::space::ChipDma,
    owner::{MacInterruptSetup, RadioAccess},
};
use oer_esp32s31_ieee80211_mac::{
    irq::{
        EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqWork,
        epoch::{ActiveEpoch, Moderated},
    },
    local_time::MacTimeline,
    rx::pool::NetworkRxFrame,
};
use oer_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
use oer_memory::HandlerCell;
use oer_radio_regulatory::{PowerLimits, RegulatoryDomain};

/// Image-owned policy and opaque operation contexts. No executor is selected.
pub trait Composition: 'static {
    type Profile: backend::BackendTxProfile;
    type Entropy: WifiTxEntropy;
    type Body: TxBody + 'static;
    type Context<'id>;
    type ConfigureContext<'id>;
}

pub type Power = crate::tx::CappedTxPowerProfile<oer_esp32s31_phy::PhyTxTargetPowerProfile>;

/// One retained original DMA allocation; normalization borrows it in place.
pub struct StagedReceive<const Q: usize, const RB: usize>(NetworkRxFrame<'static, Q, RB>);
impl<const Q: usize, const RB: usize> ReceiveUnit for StagedReceive<Q, RB> {
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError> {
        self.0.normalized(W1_INGRESS)
    }
    fn bytes(&self) -> &[u8] {
        self.0.segment().buffer
    }
}

struct Received<const Q: usize, const RB: usize>(
    RefCell<heapless::Deque<NetworkRxFrame<'static, Q, RB>, Q>>,
);
impl<const Q: usize, const RB: usize> Publisher<'static, RB, Q> for Received<Q, RB> {
    const DEPTH: usize = Q;
    fn free_capacity(&self) -> usize {
        Q - self.0.borrow().len()
    }
    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        frame_control_preview(unit, bytes)
    }
    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        unclassified_preview(unit)
    }
    fn try_send(
        &self,
        frame: NetworkRxFrame<'static, Q, RB>,
    ) -> Result<(), NetworkRxFrame<'static, Q, RB>> {
        self.0.borrow_mut().push_back(frame)
    }
}

struct Active<W> {
    registers: RadioAccess,
    platform: W,
    context: WifiRuntimeContext,
}

/// The image supplies one hardware partition, actor cell, static RX arena,
/// routes and doorbell. Every one stays with this task-side owner.
pub struct RadioDevice<
    C: Composition,
    T: Timer,
    W,
    R,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
> {
    active: Option<Active<W>>,
    session: Result<ReceiveSession<N, RB, RS, Q>, Fault>,
    setup: Option<MacInterruptSetup>,
    actor: Option<MembershipCell<Wifi>>,
    retune: Option<WifiRetunePending<W>>,
    failed_retune: Option<WifiRetuneFailure<W>>,
    received: Received<Q, RB>,
    timer: T,
    routes: R,
    doorbell: D,
    domain: RegulatoryDomain,
    limits: PowerLimits,
    timeline: MacTimeline,
    power: Option<Power>,
    rx_pending: bool,
    tx_pending: u32,
    tbtt_pending: bool,
    cut: bool,
    pausing: bool,
    continuation: Option<Instant>,
    streak: u32,
    marker: PhantomData<fn() -> C>,
}

pub struct RadioParts<T, W, R, D, const N: usize, const RB: usize, const RS: usize, const Q: usize>
{
    pub wifi: WifiStopped<W>,
    pub rx: WifiRxParts<'static, N, RB, RS, Q, ChipDma, ActiveEpoch<Moderated>>,
    pub interrupt_cell: &'static HandlerCell<WifiIrqLine<ActiveEpoch<Moderated>>>,
    pub actor: MembershipCell<Wifi>,
    pub timer: T,
    pub routes: R,
    pub doorbell: D,
    pub domain: RegulatoryDomain,
    pub power_limits: PowerLimits,
}
impl<
    C: Composition,
    T: Timer,
    W,
    R,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
> RadioDevice<C, T, W, R, D, N, RB, RS, Q>
{
    pub fn new(parts: RadioParts<T, W, R, D, N, RB, RS, Q>) -> Self {
        let WifiRuntimeParts {
            platform,
            mut registers,
            interrupt_setup,
            context,
        } = parts.wifi.into_runtime_parts();
        let session = ReceiveSession::new(
            parts.rx,
            interrupt_setup,
            &mut registers,
            parts.interrupt_cell,
        )
        .map_err(Fault::Receive);
        Self {
            active: Some(Active {
                registers,
                platform,
                context,
            }),
            session,
            setup: None,
            actor: Some(parts.actor),
            retune: None,
            failed_retune: None,
            received: Received(RefCell::new(heapless::Deque::new())),
            timer: parts.timer,
            routes: parts.routes,
            doorbell: parts.doorbell,
            domain: parts.domain,
            limits: parts.power_limits,
            timeline: MacTimeline::new(),
            power: None,
            rx_pending: false,
            tx_pending: 0,
            tbtt_pending: false,
            cut: false,
            pausing: false,
            continuation: None,
            streak: 0,
            marker: PhantomData,
        }
    }

    /// Recover the exact Wi-Fi frontier after the port's normal uninstall.
    /// A live or failed device retains all its owners and returns `None`.
    pub fn take_stopped(&mut self) -> Option<(WifiStopped<W>, MembershipCell<Wifi>)> {
        if self.failed_retune.is_some() || self.retune.is_some() {
            return None;
        }
        let setup = self.setup.take()?;
        let Active {
            registers,
            platform,
            context,
        } = self.active.take()?;
        Some((
            context.into_stopped(platform, registers, setup),
            self.actor
                .take()
                .expect("stopped device retains actor cell"),
        ))
    }
}

impl<
    C,
    T,
    W,
    R,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
    const B: usize,
> Device<B> for RadioDevice<C, T, W, R, D, N, RB, RS, Q>
where
    C: Composition,
    T: Timer + Clone + 'static,
    W: WifiMacPlatform + 'static,
    R: WifiIrqRoutes + 'static,
    D: RadioDoorbell + 'static,
{
    type Profile = C::Profile;
    type Power = Power;
    type Entropy = C::Entropy;
    type Timer = T;
    type Hardware = RadioAccess;
    type Receive = StagedReceive<Q, RB>;
    type Body = C::Body;
    type Context<'id> = C::Context<'id>;
    type ConfigureContext<'id> = C::ConfigureContext<'id>;
    fn hardware(&mut self) -> &mut RadioAccess {
        &mut self
            .active
            .as_mut()
            .expect("hardware is lent only outside an actor transaction")
            .registers
    }
    fn timer(&self) -> &T {
        &self.timer
    }
    fn next_deadline(&self) -> Option<Instant> {
        [
            self.session
                .as_ref()
                .ok()
                .and_then(ReceiveSession::port_deadline),
            self.continuation,
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn take_power_profile(&mut self) -> Option<Power> {
        self.power.take()
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, ClockError> {
        let active = self.active.as_mut().ok_or(ClockError::Unavailable)?;
        Ok(Ieee80211Instant::from_micros(
            self.timeline
                .advance(active.registers.wifi_mac_hal().mac_local_time()),
        ))
    }
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: oer_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        self.pausing = false;
        if let Some(retune) = self.retune.as_mut() {
            let (cell, result) = core::task::ready!(Pin::new(retune).poll(cx));
            self.retune = None;
            self.actor = Some(cell);
            match result {
                Ok((wifi, report)) => {
                    let WifiRuntimeParts {
                        platform,
                        registers,
                        interrupt_setup,
                        context,
                    } = wifi.into_runtime_parts();
                    let permit = self.domain.permit(channel).map_err(|_| Fault::Capacity)?;
                    self.power = Some(Power::new(report.tx_power, permit.tx_ceiling(self.limits)));
                    self.active = Some(Active {
                        registers,
                        platform,
                        context,
                    });
                    self.setup = Some(interrupt_setup);
                }
                Err(failure) => {
                    self.failed_retune = Some(failure);
                    return Poll::Ready(Err(Fault::Abandoned));
                }
            }
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        if active.context.current_channel() != channel {
            if self.setup.is_none() {
                self.setup = Some(
                    core::task::ready!(session.port_pause(
                        cx,
                        &mut active.registers,
                        &mut self.routes,
                        &self.doorbell,
                        &self.timer
                    ))
                    .map_err(Fault::Receive)?,
                );
            }
            let permit = self.domain.permit(channel).map_err(|_| Fault::Capacity)?;
            let ceiling = permit.tx_ceiling(self.limits);
            let Active {
                registers,
                platform,
                context,
            } = self.active.take().expect("paused radio retained");
            let wifi = context.into_stopped(
                platform,
                registers,
                self.setup.take().expect("pause returned interrupt setup"),
            );
            self.retune = Some(wifi.start_retune(
                self.actor.take().expect("actor cell retained"),
                permit,
                ceiling,
            ));
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if let Some(setup) = self.setup.take() {
            session.reattach(setup).map_err(Fault::Receive)?;
        }
        session
            .port_start(&mut active.registers, &self.timer, &mut self.routes)
            .map(|r| r.map_err(Fault::Receive))
    }
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        self.pausing = true;
        if self.setup.is_some() {
            return Poll::Ready(Ok(()));
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        while let Some(frame) = self.received.0.borrow_mut().pop_front() {
            drop(frame);
        }
        let setup = core::task::ready!(session.port_pause(
            cx,
            &mut active.registers,
            &mut self.routes,
            &self.doorbell,
            &self.timer
        ))
        .map_err(Fault::Receive)?;
        self.setup = Some(setup);
        self.rx_pending = false;
        self.cut = false;
        Poll::Ready(Ok(()))
    }
    fn poll_receive(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<Self::Receive>, Fault>> {
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        if receive_room != 0 {
            if let Some(frame) = self.received.0.borrow_mut().pop_front() {
                return Poll::Ready(Ok(DeviceEvent::Receive(StagedReceive(frame))));
            }
        }
        if self.cut || self.pausing {
            return Poll::Pending;
        }
        for _ in 0..4 {
            match session.port_power(cx) {
                Poll::Ready(Ok(facts)) => self.tbtt_pending |= facts.sta_tbtt(),
                Poll::Ready(Err(cause)) => return Poll::Ready(Err(Fault::Receive(cause))),
                Poll::Pending => break,
            }
        }
        if let Poll::Ready(facts) = session.poll(cx) {
            let mut facts = facts.map_err(Fault::Receive)?;
            self.rx_pending |= facts.rx_capacity_returned();
            for work in &mut facts {
                match work {
                    IrqWork::RxSuccess => self.rx_pending = true,
                    IrqWork::TxComplete => self.tx_pending |= EVENT_TX_COMPLETE,
                    IrqWork::TxTimeout => self.tx_pending |= EVENT_TX_TIMEOUT,
                    _ => {}
                }
            }
        }
        if self.rx_pending
            && receive_room != 0
            && self.continuation.is_none_or(|at| self.timer.now() >= at)
        {
            self.continuation = None;
            let progress = session
                .port_service(
                    &mut active.registers,
                    &mut self.routes,
                    &self.doorbell,
                    &self.timer,
                    &self.received,
                )
                .map_err(Fault::Receive)?;
            use crate::datapath::DatapathRxProgress as Progress;
            match progress {
                Progress::Drained | Progress::UpperLayerBlockedButDroppable => {
                    self.rx_pending = false;
                    self.streak = 0;
                    session.port_unmask(&self.doorbell);
                }
                Progress::StageCapacityBlocked => {}
                _ => {
                    if self.streak < IMMEDIATE_CONTINUATIONS {
                        self.streak += 1;
                        cx.waker().wake_by_ref();
                    } else {
                        self.streak = 0;
                        self.continuation =
                            Some(self.timer.now().saturating_add(CONTINUATION_WAIT));
                    }
                }
            }
            if let Some(frame) = self.received.0.borrow_mut().pop_front() {
                return Poll::Ready(Ok(DeviceEvent::Receive(StagedReceive(frame))));
            }
        }
        if self.tx_pending != 0 {
            let events = core::mem::take(&mut self.tx_pending);
            return Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events,
            })));
        }
        if core::mem::take(&mut self.tbtt_pending) {
            return Poll::Ready(Ok(DeviceEvent::Tbtt));
        }
        Poll::Pending
    }
    fn poll_rx_cut(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Self::Receive>, Fault>> {
        self.cut = true;
        if receive_room != 0 {
            if let Some(frame) = self.received.0.borrow_mut().pop_front() {
                return Poll::Ready(Ok(Some(StagedReceive(frame))));
            }
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        core::task::ready!(session.port_freeze(
            cx,
            &mut active.registers,
            &mut self.routes,
            &self.doorbell,
            &self.timer
        ))
        .map_err(Fault::Receive)?;
        self.cut = true;
        if receive_room == 0 {
            return Poll::Pending;
        }
        if self.continuation.is_some_and(|at| self.timer.now() < at) {
            return Poll::Pending;
        }
        self.continuation = None;
        let progress = session
            .port_service(
                &mut active.registers,
                &mut self.routes,
                &self.doorbell,
                &self.timer,
                &self.received,
            )
            .map_err(Fault::Receive)?;
        if let Some(frame) = self.received.0.borrow_mut().pop_front() {
            return Poll::Ready(Ok(Some(StagedReceive(frame))));
        }
        if matches!(
            progress,
            crate::datapath::DatapathRxProgress::Drained
                | crate::datapath::DatapathRxProgress::UpperLayerBlockedButDroppable
        ) {
            self.streak = 0;
            Poll::Ready(Ok(None))
        } else if matches!(
            progress,
            crate::datapath::DatapathRxProgress::StageCapacityBlocked
        ) {
            Poll::Pending
        } else {
            if self.streak < IMMEDIATE_CONTINUATIONS {
                self.streak += 1;
                cx.waker().wake_by_ref();
            } else {
                self.streak = 0;
                self.continuation = Some(self.timer.now().saturating_add(CONTINUATION_WAIT));
            }
            Poll::Pending
        }
    }
    fn recycle(&mut self, frame: Self::Receive) {
        drop(frame);
        self.rx_pending = true;
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        session
            .port_resume_cut(&mut active.registers, &mut self.routes)
            .map_err(Fault::Receive)?;
        self.cut = false;
        self.continuation = None;
        self.streak = 0;
        self.rx_pending = true;
        Ok(())
    }
    fn abandon(&mut self) -> bool {
        if let Ok(session) = self.session.as_mut() {
            session.port_abandon(&self.doorbell);
        }
        self.routes.silence();
        if let Some(active) = self.active.as_mut() {
            oer_esp32s31_ieee80211_mac::crypto::erase_lower_mac_keys(&mut active.registers);
        }
        // Masking a route is not a walker/entered-ISR stop proof.
        false
    }
    fn detach(&mut self) {
        self.routes.silence();
        if let Some(active) = self.active.as_mut() {
            oer_esp32s31_ieee80211_mac::crypto::erase_lower_mac_keys(&mut active.registers);
        }
    }
}
