//! The cooperative S31 register/RX owner used by the port driver.
use super::*;
use crate::mac::{
    irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqWork},
    local_time::MacTimeline,
    rx::pool::NetworkRxFrame,
};
use crate::{
    cooperative_hardware::ReceiveRegisters,
    irq::{RadioDoorbell, WifiIrqActivation, WifiIrqLine},
    rx::{
        arena::WifiRxParts,
        receive::{CONTINUATION_WAIT, IMMEDIATE_CONTINUATIONS, RecycledProbe, ReturnQuiet},
        session::ReceiveSession,
        sink::{frame_control_preview, unclassified_preview},
        transaction::{CompletedUnit, Preview, Publisher},
        upper::W1_INGRESS,
    },
};
use core::cell::RefCell;
use oer_chips_esp32s31_hal::radio::ieee80211::dma::space::DmaSpace;
use oer_chips_family_platform_interrupt_table::signal::SignalExecutor;
use oer_portable_memory::HandlerCell;
use oer_protocols_ieee80211_wire::channel::WifiChannel;
use oer_protocols_radio_regulatory::{Permitted, PowerLimits, RegulatoryDomain, TxCeiling};

/// Image-owned policy and opaque operation contexts. No executor is selected.
pub trait Composition: 'static {
    type Coex: RadioCoexPriorities + 'static;
    type Entropy: WifiTxEntropy;
    type Body: TxBody + 'static;
    type OwnedContext<Id: SlotIdentity>;
    type Context<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedContext<Id>>
        + 'p;
    type OwnedConfigureContext<Id: SlotIdentity>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedConfigureContext<Id>>
        + 'p;
}

/// The transmit power of a device over the frontier `F`.
pub type Power<F> = crate::tx::CappedTxPowerProfile<<F as Frontier>::Calibrated>;

/// The stopped radio's owner a device runs from and returns to: what it
/// splits into while the device runs and how it moves between channels. The
/// chip's is [`WifiStopped`](crate::runtime::WifiStopped).
pub trait Frontier: Sized + 'static {
    /// The registers, while the device holds them.
    type Registers: LowerMacHardware + ReceiveRegisters + 'static;
    /// The DMA space of the receive ring.
    type Space: DmaSpace;
    /// The calibrated transmit power a retune reports.
    type Calibrated: WifiTxPowerProfile;
    /// What stays beside the registers while the device runs.
    type Context;
    /// The device's cell of the radio's actor.
    type Cell;
    /// A retune in flight, kept in the device across polls.
    type Retune: Unpin;
    /// The owner of a retune that left the radio poisoned.
    type Failed;
    fn into_running(self) -> (Self::Registers, Self::Context);
    /// The stopped owner of the registers `context` went with.
    fn into_stopped(registers: Self::Registers, context: Self::Context) -> Self;
    /// The channel the radio is tuned to.
    fn current_channel(context: &Self::Context) -> WifiChannel;
    /// Ask the radio's actor to retune; the stopped owner goes with the
    /// request.
    fn start_retune(
        self,
        cell: Self::Cell,
        channel: Permitted<RegulatedChannel<WifiChannel>>,
        ceiling: TxCeiling,
    ) -> Self::Retune;
    fn poll_retune(retune: &mut Self::Retune, cx: &mut Context<'_>) -> Poll<RetuneEnd<Self>>;
}

/// How a retune ended ([`Frontier::poll_retune`]).
pub enum RetuneEnd<F: Frontier> {
    /// The radio is on the new channel.
    Moved {
        stopped: F,
        cell: F::Cell,
        tx_power: F::Calibrated,
    },
    /// The retune failed and the radio changed nothing: the owner stays on
    /// its channel.
    Unchanged {
        stopped: F,
        cell: F::Cell,
        cause: WifiRetuneError,
    },
    /// The retune failed and only a release remains.
    Poisoned {
        failed: F::Failed,
        cell: F::Cell,
        cause: WifiRetuneError,
    },
    /// The radio's actor never answered; what it held stays quarantined.
    Abandoned,
}

/// A blocked receive pull needs the caller to return a unit. Its
/// continuation must not keep the caller's alarm at an already reached
/// deadline while it cannot take one.
fn suspend_rx_continuation(receive_room: usize, continuation: &mut Option<Instant>) {
    if receive_room == 0 {
        *continuation = None;
    }
}

/// One retained original DMA allocation; normalization borrows it in place.
pub struct StagedReceive<const Q: usize, const RB: usize>(NetworkRxFrame<'static, Q, RB>);
impl<const Q: usize, const RB: usize> ReceiveUnit for StagedReceive<Q, RB> {
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError> {
        super::receive::normalize(self.0.segment(), W1_INGRESS)
    }
    fn bytes(&self) -> &[u8] {
        self.0.segment().buffer
    }
    fn narrow(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<(), oer_portable_memory::FrameRangeError> {
        self.0.narrow(offset, length)
    }
    fn storage(&self) -> &[u8] {
        self.0.storage()
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        self.0.storage_mut()
    }
}

struct Received<const Q: usize, const RB: usize>(
    RefCell<heapless::Deque<NetworkRxFrame<'static, Q, RB>, Q>>,
);
impl<const Q: usize, const RB: usize> Publisher<'static, RB, Q> for Received<Q, RB> {
    const DEPTH: usize = Q;
    fn free_capacity(&self) -> usize {
        // A queue borrowed by a send in progress has no observable room.
        self.0
            .try_borrow()
            .map_or(0, |queue| Q.saturating_sub(queue.len()))
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
        match self.0.try_borrow_mut() {
            Ok(mut queue) => queue.push_back(frame),
            // The frame stays the caller's.
            Err(_) => Err(frame),
        }
    }
}

struct Active<F: Frontier> {
    registers: F::Registers,
    context: F::Context,
}

/// A trace build's stall snapshots ([`RxStall`](crate::rx::counters::stall::RxStall)):
/// the record a diagnostic reader asks through, the last ticket served,
/// the ticket whose snapshot waits for its turn's receive pull and whether
/// that pull ran, and how far the ring's descriptors and the units handed
/// on went.
#[cfg(oer_trace)]
struct StallOwner {
    record: &'static crate::rx::counters::stall::RxStall,
    served: u32,
    after: Option<crate::rx::counters::stall::RxStallTicket>,
    pulled: bool,
    dma: crate::rx::counters::stall::RxProgress,
    driver: crate::rx::counters::stall::RxProgress,
}

/// The low word of `at`'s microseconds: a stall record's time.
#[cfg(oer_trace)]
fn low_micros(at: Instant) -> u32 {
    at.as_micros() as u32
}

/// The image supplies one hardware partition, actor cell, static RX arena,
/// the router's proof that the image binds the radio handler's part (`V`,
/// a [`Bound`](oer_chips_family_platform_interrupt_table::Bound) of a
/// [`WifiIrqPart`](crate::irq::WifiIrqPart)) and the doorbell.
/// Every one stays with this task-side owner.
pub struct RadioDevice<
    C: Composition,
    T: Timer,
    F: Frontier,
    V: WifiIrqActivation,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
    X: SignalExecutor,
> {
    active: Option<Active<F>>,
    session: Result<ReceiveSession<N, RB, RS, Q, X, V, F::Space>, Fault>,
    /// The receive session is paused and its interrupt epoch closed.
    paused: bool,
    actor: Option<F::Cell>,
    retune: Option<F::Retune>,
    failed_retune: Option<F::Failed>,
    received: Received<Q, RB>,
    timer: T,
    doorbell: D,
    domain: RegulatoryDomain,
    limits: PowerLimits,
    timeline: MacTimeline,
    power: Option<Power<F>>,
    rx_pending: bool,
    /// The receive signal held facts at the last facts call: ring work its
    /// next pull takes.
    rx_raised: bool,
    tx_pending: u32,
    tbtt_pending: bool,
    cut: bool,
    /// The cut's last service found no staging place free.
    cut_blocked: bool,
    pausing: bool,
    continuation: Option<Instant>,
    streak: u32,
    /// The wait of the drain's recycled appends.
    recycled: RecycledProbe,
    returns: ReturnQuiet,
    #[cfg(oer_trace)]
    stall: StallOwner,
    marker: PhantomData<fn() -> C>,
}

pub struct RadioParts<
    T,
    F: Frontier,
    V: WifiIrqActivation,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
    X: SignalExecutor,
> {
    pub wifi: F,
    pub rx: WifiRxParts<'static, N, RB, RS, Q, F::Space, X>,
    pub interrupt_cell: &'static HandlerCell<WifiIrqLine<X>>,
    /// The router's proof that the image binds the radio handler's part.
    pub bound: V,
    pub actor: F::Cell,
    pub timer: T,
    pub doorbell: D,
    pub domain: RegulatoryDomain,
    pub power_limits: PowerLimits,
    /// A trace build's stall record, which a diagnostic reader asks
    /// through.
    #[cfg(oer_trace)]
    pub stall: &'static crate::rx::counters::stall::RxStall,
}
impl<
    C: Composition,
    T: Timer,
    F: Frontier,
    V: WifiIrqActivation,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
    X: SignalExecutor,
> RadioDevice<C, T, F, V, D, N, RB, RS, Q, X>
{
    /// RX stays masked after a pass that left `progress`: whether the ring
    /// runs again at once, or else its continuation. Out of line: a turn
    /// without received frames carries none of this.
    #[inline(never)]
    fn keep_masked(&mut self, progress: crate::datapath::DatapathRxProgress, now: Instant) -> bool {
        use crate::datapath::DatapathRxProgress as Progress;
        let wait = if progress == Progress::RecycledAppendPending {
            let frames = self.received.0.get_mut();
            let bytes = frames.iter().fold(0_usize, |sum, frame| {
                sum.saturating_add(frame.segment().buffer.len())
            });
            self.recycled.wait(frames.len(), bytes)
        } else {
            self.recycled.reset();
            None
        };
        if let Some(wait) = wait {
            // The wait lets the ring fill behind the mask.
            self.streak = 0;
            self.continuation = Some(now.saturating_add(wait));
            false
        } else if self.streak < IMMEDIATE_CONTINUATIONS {
            self.streak += 1;
            true
        } else {
            self.streak = 0;
            self.continuation = Some(now.saturating_add(CONTINUATION_WAIT));
            false
        }
    }

    pub fn new(parts: RadioParts<T, F, V, D, N, RB, RS, Q, X>) -> Self {
        const {
            if ReturnQuiet::LIMIT as usize > Q {
                panic!("a pass blocked on staging capacity hears its returns");
            }
        }
        let (mut registers, context) = parts.wifi.into_running();
        let session =
            ReceiveSession::new(parts.rx, parts.bound, &mut registers, parts.interrupt_cell)
                .map_err(Fault::Receive);
        Self {
            active: Some(Active { registers, context }),
            session,
            paused: false,
            actor: Some(parts.actor),
            retune: None,
            failed_retune: None,
            received: Received(RefCell::new(heapless::Deque::new())),
            timer: parts.timer,
            doorbell: parts.doorbell,
            domain: parts.domain,
            limits: parts.power_limits,
            timeline: MacTimeline::new(),
            power: None,
            rx_pending: false,
            rx_raised: false,
            tx_pending: 0,
            tbtt_pending: false,
            cut: false,
            cut_blocked: false,
            pausing: false,
            continuation: None,
            streak: 0,
            recycled: RecycledProbe::default(),
            returns: ReturnQuiet::default(),
            #[cfg(oer_trace)]
            stall: StallOwner {
                record: parts.stall,
                // A request older than this device was another's to serve.
                served: parts.stall.requested(0).map_or(0, |ticket| ticket.get()),
                after: None,
                pulled: false,
                dma: crate::rx::counters::stall::RxProgress::default(),
                driver: crate::rx::counters::stall::RxProgress::default(),
            },
            marker: PhantomData,
        }
    }

    /// A staged unit leaves for the upper layer, at `now` when the caller
    /// read the time.
    fn hand_over(
        &mut self,
        frame: NetworkRxFrame<'static, Q, RB>,
        now: Option<Instant>,
    ) -> StagedReceive<Q, RB> {
        self.returns.received();
        #[cfg(oer_trace)]
        {
            let at = now.unwrap_or_else(|| self.timer.now());
            self.stall.driver.advance(1, low_micros(at));
        }
        #[cfg(not(oer_trace))]
        let _ = now;
        StagedReceive(frame)
    }

    /// Serve a trace build's stall record at the start of the port's facts:
    /// register the task a request wakes; record how far the boundaries
    /// went since the last snapshot, for the last time once its turn's
    /// receive pull ran; take the snapshot a reader asked for, before any
    /// receive service of this turn. Whether it took one: the turn then runs
    /// as for a receive fact, with the device's own ring work as it was.
    #[cfg(oer_trace)]
    #[inline(never)]
    fn serve_stall(&mut self, cx: &Context<'_>) -> bool {
        use crate::rx::counters::stall::RxStallResume;
        self.stall.record.register(cx.waker());
        if let Some(ticket) = self.stall.after {
            let serviced = self.stall.pulled;
            let resume = RxStallResume {
                at: low_micros(self.timer.now()),
                dma: self.stall.dma,
                driver: self.stall.driver,
                serviced,
            };
            self.stall.record.record_resume(ticket, &resume);
            if serviced {
                self.stall.after = None;
            }
        }
        let Some(ticket) = self.stall.record.requested(self.stall.served) else {
            return false;
        };
        self.stall.served = ticket.get();
        let snapshot = self.stall_snapshot();
        self.stall.record.record_snapshot(ticket, &snapshot);
        self.stall.after = Some(ticket);
        self.stall.pulled = false;
        true
    }

    /// What the owner reads of its receive path now. Only reads.
    #[cfg(oer_trace)]
    fn stall_snapshot(&mut self) -> crate::rx::counters::stall::RxStallSnapshot {
        use crate::rx::counters::stall::{RxDeviceView, RxStallSnapshot};
        let at = low_micros(self.timer.now());
        let (hardware, (mac, periodic)) = match self.active.as_mut() {
            Some(active) => (
                self.session
                    .as_mut()
                    .ok()
                    .map(|session| session.stall_view(&mut active.registers)),
                active.registers.mac_banks(),
            ),
            None => (None, (None, None)),
        };
        RxStallSnapshot {
            at,
            dma: self.stall.dma,
            driver: self.stall.driver,
            device: RxDeviceView {
                rx_pending: self.rx_pending,
                continuation: self.continuation.is_some(),
                cut: self.cut,
                pausing: self.pausing,
                paused: self.paused,
                queued: u32::try_from(self.received.0.get_mut().len()).unwrap_or(u32::MAX),
            },
            hardware,
            mac,
            periodic,
        }
    }

    /// Recover the exact Wi-Fi frontier after the port's normal uninstall.
    /// A live or failed device retains all its owners and returns `None`.
    pub fn take_stopped(&mut self) -> Option<(F, F::Cell)> {
        if self.failed_retune.is_some() || self.retune.is_some() {
            return None;
        }
        if !self.paused {
            return None;
        }
        // Both owners leave together, or neither does.
        match (self.active.take(), self.actor.take()) {
            (Some(Active { registers, context }), Some(actor)) => {
                Some((F::into_stopped(registers, context), actor))
            }
            (active, actor) => {
                self.active = active;
                self.actor = actor;
                None
            }
        }
    }
}

impl<
    C,
    T,
    F,
    V,
    D,
    const N: usize,
    const RB: usize,
    const RS: usize,
    const Q: usize,
    const B: usize,
    X: SignalExecutor,
> Device<B> for RadioDevice<C, T, F, V, D, N, RB, RS, Q, X>
where
    C: Composition,
    T: Timer + Clone + 'static,
    F: Frontier,
    V: WifiIrqActivation + 'static,
    D: RadioDoorbell + 'static,
{
    type Power = Power<F>;
    type Coex = C::Coex;
    type Entropy = C::Entropy;
    type Timer = T;
    type Hardware = F::Registers;
    type Receive = StagedReceive<Q, RB>;
    /// The composition wrapping this device names where its network runs.
    type Lent = super::SameHart;
    const LENT_RX: usize = super::lent_rx(Q);
    type Body = C::Body;
    type OwnedContext<Id: SlotIdentity> = C::OwnedContext<Id>;
    type Context<'p, Id: SlotIdentity + 'p> = C::Context<'p, Id>;
    type OwnedConfigureContext<Id: SlotIdentity> = C::OwnedConfigureContext<Id>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p> = C::ConfigureContext<'p, Id>;
    fn hardware(&mut self) -> Result<&mut F::Registers, Fault> {
        self.active
            .as_mut()
            .map(|active| &mut active.registers)
            .ok_or(Fault::HardwareUnavailable)
    }
    fn next_deadline(&self) -> Option<Instant> {
        // A due continuation is the port's receive pull's to take: a passed
        // deadline would ask for a transmit service at every turn.
        let continuation = self.continuation.filter(|at| self.timer.now() < *at);
        [
            self.session
                .as_ref()
                .ok()
                .and_then(ReceiveSession::port_deadline),
            continuation,
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn event_waiting(&self) -> bool {
        self.session.as_ref().is_ok_and(ReceiveSession::has_event)
    }
    fn take_power_profile(&mut self) -> Option<Power<F>> {
        self.power.take()
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, ClockError> {
        let active = self.active.as_mut().ok_or(ClockError::Unavailable)?;
        Ok(Ieee80211Instant::from_micros(
            self.timeline.advance(active.registers.mac_local_time()),
        ))
    }
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        self.pausing = false;
        if let Some(retune) = self.retune.as_mut() {
            let end = core::task::ready!(F::poll_retune(retune, cx));
            self.retune = None;
            match end {
                RetuneEnd::Abandoned => return Poll::Ready(Err(Fault::Abandoned)),
                RetuneEnd::Moved {
                    stopped,
                    cell,
                    tx_power,
                } => {
                    self.actor = Some(cell);
                    let (registers, context) = stopped.into_running();
                    let permit = self
                        .domain
                        .permit(RegulatedChannel::from(channel))
                        .map_err(|_| Fault::ChannelNotPermitted)?;
                    self.power = Some(Power::<F>::new(tx_power, permit.tx_ceiling(self.limits)));
                    self.active = Some(Active { registers, context });
                    self.paused = true;
                }
                RetuneEnd::Unchanged {
                    stopped,
                    cell,
                    cause,
                } => {
                    self.actor = Some(cell);
                    // The radio changed nothing: the port stays on its
                    // channel for the caller's next start.
                    let (registers, context) = stopped.into_running();
                    self.active = Some(Active { registers, context });
                    self.paused = true;
                    return Poll::Ready(Err(Fault::Retune(cause)));
                }
                RetuneEnd::Poisoned {
                    failed,
                    cell,
                    cause,
                } => {
                    self.actor = Some(cell);
                    self.failed_retune = Some(failed);
                    return Poll::Ready(Err(Fault::Retune(cause)));
                }
            }
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        if F::current_channel(&active.context) != channel {
            if !self.paused {
                core::task::ready!(session.port_pause(
                    cx,
                    &mut active.registers,
                    &self.doorbell,
                    &self.timer
                ))
                .map_err(Fault::Receive)?;
                self.paused = true;
            }
            let permit = self
                .domain
                .permit(RegulatedChannel::from(channel))
                .map_err(|_| Fault::ChannelNotPermitted)?;
            let ceiling = permit.tx_ceiling(self.limits);
            // The paused radio and its actor cell leave together, or the
            // device keeps both and reports the missing owner.
            let (registers, context, actor) = match (self.active.take(), self.actor.take()) {
                (Some(Active { registers, context }), Some(actor)) => (registers, context, actor),
                (active, actor) => {
                    self.active = active;
                    self.actor = actor;
                    return Poll::Ready(Err(Fault::HardwareUnavailable));
                }
            };
            let wifi = F::into_stopped(registers, context);
            self.retune = Some(wifi.start_retune(actor, permit, ceiling));
            // Poll the retune at once: its future registers `cx` itself.
            return <Self as Device<B>>::poll_start(self, cx, channel);
        }
        if core::mem::take(&mut self.paused) {
            session.reattach();
        }
        session
            .port_start(&mut active.registers, &self.doorbell)
            .map(|r| r.map_err(Fault::Receive))
    }
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        self.pausing = true;
        if self.paused {
            return Poll::Ready(Ok(()));
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        while let Some(frame) = self.received.0.get_mut().pop_front() {
            drop(frame);
        }
        core::task::ready!(session.port_pause(
            cx,
            &mut active.registers,
            &self.doorbell,
            &self.timer
        ))
        .map_err(Fault::Receive)?;
        self.paused = true;
        self.rx_pending = false;
        self.cut = false;
        // The paused ring's continuation ended with it.
        self.continuation = None;
        self.streak = 0;
        Poll::Ready(Ok(()))
    }
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Result<DeviceEvent, Fault>> {
        let session = self.session.as_mut().map_err(|e| *e)?;
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
            for work in facts.map_err(Fault::Receive)? {
                match work {
                    IrqWork::TxComplete => self.tx_pending |= EVENT_TX_COMPLETE,
                    IrqWork::TxTimeout => self.tx_pending |= EVENT_TX_TIMEOUT,
                    _ => {}
                }
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
        #[cfg(oer_trace)]
        {
            self.stall.pulled = true;
        }
        self.cut = true;
        self.cut_blocked = false;
        if receive_room != 0 {
            if let Some(frame) = self.received.0.get_mut().pop_front() {
                return Poll::Ready(Ok(Some(self.hand_over(frame, None))));
            }
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        core::task::ready!(session.port_freeze(
            cx,
            &mut active.registers,
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
        #[cfg(oer_trace)]
        let observed = session.observed_descriptors();
        let progress = session
            .port_service(
                &mut active.registers,
                &self.doorbell,
                &self.timer,
                &self.timer,
                &self.received,
            )
            .map_err(Fault::Receive)?;
        #[cfg(oer_trace)]
        self.stall.dma.advance(
            u32::try_from(session.observed_descriptors().saturating_sub(observed))
                .unwrap_or(u32::MAX),
            low_micros(self.timer.now()),
        );
        if let Some(frame) = self.received.0.get_mut().pop_front() {
            return Poll::Ready(Ok(Some(self.hand_over(frame, None))));
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
            self.cut_blocked = true;
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
    fn cut_held_by_units(&self) -> bool {
        self.cut_blocked
    }
    fn returns_quiet(&mut self, held: usize, returned: bool) -> bool {
        let rearm_due = returned || self.rx_pending || self.rx_raised;
        let whole = self.returns.quiet(held, rearm_due);
        whole || self.continuation.is_some()
    }
    fn poll_rx_work(&mut self, cx: &mut Context<'_>) -> Result<bool, Fault> {
        // A trace build's stall snapshot goes before any of this turn's
        // receive work, the pull's take of the receive facts included.
        #[cfg(oer_trace)]
        let diagnosed = self.serve_stall(cx);
        #[cfg(not(oer_trace))]
        let diagnosed = false;
        self.rx_raised = false;
        if self.cut || self.pausing {
            return Ok(diagnosed);
        }
        #[cfg(oer_trace)]
        if let Some(active) = self.active.as_mut() {
            active
                .registers
                .sample_mac_counts(self.timer.now().as_micros() as u32);
        }
        let session = self.session.as_mut().map_err(|e| *e)?;
        // The facts stay raised for the pull that serves them: taking one
        // here would take the wake its raise owes this task, and a caller
        // that does not pull before it sleeps would leave the ring masked
        // with nothing to wake it.
        if let Poll::Ready(result) = session.poll_received_raised(cx) {
            result.map_err(Fault::Receive)?;
            self.rx_raised = true;
        }
        // Ring work that waits for its continuation is the caller's alarm's
        // (the pull answered `Later`): it is no fact before then.
        Ok(diagnosed
            || !self.received.0.get_mut().is_empty()
            || ((self.rx_pending || self.rx_raised) && self.continuation.is_none()))
    }
    fn pull_unit(
        &mut self,
        receive_room: usize,
        now: Instant,
    ) -> Result<DevicePull<Self::Receive>, Fault> {
        #[cfg(oer_trace)]
        {
            self.stall.pulled = true;
        }
        suspend_rx_continuation(receive_room, &mut self.continuation);
        if receive_room == 0 || self.cut || self.pausing {
            return Ok(DevicePull::Empty);
        }
        // The pull serves the receive facts, so it takes them: a received
        // unit and a returned staging credit are both ring work.
        let facts = self
            .session
            .as_mut()
            .map_err(|e| *e)?
            .take_received()
            .map_err(Fault::Receive)?;
        self.rx_pending |= !facts.is_empty();
        self.rx_raised = false;
        if let Some(frame) = self.received.0.get_mut().pop_front() {
            return Ok(DevicePull::Unit(self.hand_over(frame, Some(now))));
        }
        if !self.rx_pending {
            return Ok(DevicePull::Empty);
        }
        if let Some(at) = self.continuation.filter(|at| now < *at) {
            return Ok(DevicePull::Later(at));
        }
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        self.continuation = None;
        // The ring is serviced at the turn's instant, not a fresh reading.
        let clock = TurnClock {
            now,
            timer: &self.timer,
        };
        #[cfg(oer_trace)]
        let observed = session.observed_descriptors();
        let progress = session
            .port_service(
                &mut active.registers,
                &self.doorbell,
                &clock,
                &self.timer,
                &self.received,
            )
            .map_err(Fault::Receive)?;
        #[cfg(oer_trace)]
        self.stall.dma.advance(
            u32::try_from(session.observed_descriptors().saturating_sub(observed))
                .unwrap_or(u32::MAX),
            low_micros(now),
        );
        use crate::datapath::DatapathRxProgress as Progress;
        let mut again = false;
        match progress {
            Progress::Drained | Progress::UpperLayerBlockedButDroppable => {
                self.rx_pending = false;
                self.streak = 0;
                self.recycled.reset();
                session.port_unmask(&self.doorbell);
            }
            Progress::StageCapacityBlocked => self.recycled.reset(),
            progress => again = self.keep_masked(progress, now),
        }
        Ok(match self.received.0.get_mut().pop_front() {
            Some(frame) => DevicePull::Unit(self.hand_over(frame, Some(now))),
            None if again => DevicePull::Again,
            None => self
                .continuation
                .map_or(DevicePull::Empty, DevicePull::Later),
        })
    }
    fn recycle(&mut self, frame: Self::Receive) {
        drop(frame);
        self.rx_pending = true;
    }
    fn returned(&mut self) {
        self.rx_pending = true;
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        let active = self.active.as_mut().ok_or(Fault::Abandoned)?;
        let session = self.session.as_mut().map_err(|e| *e)?;
        session
            .port_resume_cut(&mut active.registers, &self.doorbell)
            .map_err(Fault::Receive)?;
        self.cut = false;
        self.continuation = None;
        self.streak = 0;
        self.rx_pending = true;
        Ok(())
    }
    fn detach(&mut self) {
        if let Ok(session) = self.session.as_ref() {
            session.port_mask_all(&self.doorbell);
        }
        if let Some(active) = self.active.as_mut() {
            crate::mac::crypto::erase_lower_mac_keys(&mut active.registers);
        }
    }
}

/// The turn's one instant, as the data port's receive service reads the
/// clock; waiting and the reload settle still use the device's timer.
struct TurnClock<'a, T> {
    now: Instant,
    timer: &'a T,
}

impl<T: Timer> oer_portable_time::Clock for TurnClock<'_, T> {
    fn now(&self) -> Instant {
        self.now
    }
}

impl<T: Timer> Timer for TurnClock<'_, T> {
    fn wait_until(&self, deadline: Instant) -> impl core::future::Future<Output = ()> {
        self.timer.wait_until(deadline)
    }
}

#[cfg(test)]
mod tests;
