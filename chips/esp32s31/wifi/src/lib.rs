#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
//! ESP32-S31 station composition. Installation owners stay in its runners.
mod credentials;
pub mod failure;
pub mod observation;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
pub use credentials::{
    Callbacks, CredentialsError, NonceExhausted, NonceSource, OpenCallbacks, PreparedCredentials,
    StationSettings,
};
use oer_ieee80211_core_roles_sta::data::TxMsdu;
use oer_ieee80211_runtime::station as runtime;
use oer_memory::{QueueNotify as _, WakingSpscQueue};
use oer_network_port::{EthernetFrame, LinkSnapshot, NetDevice, TransmitRefused};
pub use runtime::StopPolicy;
pub use runtime::{ControlError, OperationEnd};

/// A station request's terminal ([`Control::connect`]).
pub type Operation<'a, N = SameHart> = runtime::Operation<'a, <N as Notifies>::Channels>;

/// Where the station's network end runs and how its ends tell each other,
/// in one type the image's binding names: [`SameHart`], or
/// [`AcrossHarts`] for a network on another hart than the station's. Every
/// notify is a type, so every call across harts is direct.
pub trait Notifies: 'static {
    /// The station's network channels.
    type Channels: runtime::Notifies;
    /// The host build's receive buffer, which it never lends.
    #[cfg(not(target_arch = "riscv32"))]
    type LentRx: oer_ieee80211_contracts_port::LentRxBuffer + Send + 'static;
    /// How a lent buffer's return tells the driver and the station's stop.
    #[cfg(target_arch = "riscv32")]
    type Lent: oer_esp32s31_ieee80211_driver::port::LentNotifies;
}

/// The station and its network end on one hart: local wakers throughout.
pub struct SameHart;

impl Notifies for SameHart {
    type Channels = runtime::SameHart;
    #[cfg(not(target_arch = "riscv32"))]
    type LentRx = Unlent;
    #[cfg(target_arch = "riscv32")]
    type Lent = oer_esp32s31_ieee80211_driver::port::SameHart;
}

/// The network end on another hart: `N` raises the network task's executor
/// signal, `S` the station task's and `D` the radio driver's. No waker
/// crosses harts; each signal wakes the one task that polls its ends.
pub struct AcrossHarts<N, S, D>(core::marker::PhantomData<Three<N, S, D>>);
/// The three notify types, named without owning a value of any.
type Three<N, S, D> = fn() -> (N, S, D);

impl<N, S, D> Notifies for AcrossHarts<N, S, D>
where
    N: oer_memory::QueueNotify + Copy + 'static,
    S: oer_memory::QueueNotify + Copy + 'static,
    D: oer_memory::QueueNotify + Copy + 'static,
{
    type Channels = runtime::AcrossHarts<N, S>;
    #[cfg(not(target_arch = "riscv32"))]
    type LentRx = Unlent;
    #[cfg(target_arch = "riscv32")]
    type Lent = oer_esp32s31_ieee80211_driver::port::NetworkPeers<D, S>;
}

/// The channels' notify types of `N`.
type Ch<N> = <N as Notifies>::Channels;

/// The station's received MSDUs waiting for the network: two receive Block
/// Ack windows, so one window's burst fits while the network takes the
/// previous one (a queue of one window dropped a 16-MPDU aggregate's tail).
pub const RX_DEPTH: usize = 32;
pub const FRAME_CAPACITY: usize = 1664;
#[cfg(target_arch = "riscv32")]
type Caps = oer_esp32s31_ieee80211_driver::port::Esp32s31Caps<FRAME_CAPACITY>;
#[cfg(not(target_arch = "riscv32"))]
type Caps = ();
/// The station's exit, as its runner publishes it.
#[cfg(target_arch = "riscv32")]
pub type Exit = chip::StationExit;
#[cfg(not(target_arch = "riscv32"))]
pub type Exit = core::convert::Infallible;

/// The sole product command end. Its type carries no port identity or loan.
pub struct Control<'a, N: Notifies = SameHart> {
    start: &'a StartState,
    stop: &'a StopState<Exit>,
    inner: runtime::Control<'a, Caps, 2, Ch<N>>,
}
impl<'a, N: Notifies> Control<'a, N> {
    /// Wait for station construction before starting an image's workload.
    pub async fn wait_started(&mut self) -> Result<(), StartError> {
        core::future::poll_fn(|cx| {
            self.start.waker.register(cx.waker());
            match self.start.result.get() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await
    }
    pub fn status_reader(&self) -> StatusReader<'a, N> {
        StatusReader {
            inner: self.inner.status_reader(),
        }
    }
    pub fn status(&self) -> runtime::Status {
        self.inner.status()
    }
    pub fn link(&self) -> LinkSnapshot {
        self.inner.status().link
    }
    pub fn stopped(&self) -> bool {
        self.inner.status().stopped
    }
    /// Admit a connect to the network `credentials` name once the previous
    /// connect's terminal returned its permission; the operation resolves
    /// exactly once to this request's terminal. Dropping it detaches the
    /// observer: the request continues. Only an idle station takes the
    /// network; it keeps it for its reconnections. A refused, stopped or
    /// cancelled connect drops the credentials, erasing the PMK.
    pub async fn connect(
        &mut self,
        credentials: PreparedCredentials,
    ) -> Result<Operation<'a, N>, ControlError> {
        self.inner.connect(credentials.into_profile()).await
    }
    /// Admit a disconnect, as [`Self::connect`].
    pub async fn disconnect(&mut self) -> Result<Operation<'a, N>, ControlError> {
        self.inner.disconnect().await
    }
    /// Admit a background scan, as [`Self::connect`].
    pub async fn scan(&mut self) -> Result<Operation<'a, N>, ControlError> {
        self.inner.scan().await
    }
    /// Disconnect and stop. The network asked through its
    /// [`NetworkStopper`] must return the station's lent receive buffers by
    /// `deadline`, or the radio does not retire
    /// ([`RetirementCause::NetworkDrain`](crate::RetirementCause::NetworkDrain)).
    pub fn stop(&mut self, deadline: oer_time::Instant) {
        self.inner.stop_gracefully(deadline);
    }
    /// The station's exit, published by its runner after the radio release
    /// ended. The copy reports the result; the runner keeps every owner.
    pub async fn wait_stopped(&mut self) -> Result<Exit, RunnerLost> {
        self.stop.wait().await
    }
}
/// The station runner ended without publishing its exit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunnerLost;
/// The one exit a runner publishes, and its waiter.
struct StopState<E> {
    result: core::cell::Cell<Option<Result<E, RunnerLost>>>,
    waker: oer_sync::PriorityWaker,
}
impl<E: Copy> StopState<E> {
    const fn new() -> Self {
        Self {
            result: core::cell::Cell::new(None),
            waker: oer_sync::PriorityWaker::new(),
        }
    }
    async fn wait(&self) -> Result<E, RunnerLost> {
        core::future::poll_fn(|cx| {
            self.waker.register(cx.waker());
            match self.result.get() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await
    }
}
/// The runner's affine duty to publish its exit; losing it publishes
/// [`RunnerLost`].
pub(crate) struct StopGuard<'a, E: Copy>(&'a StopState<E>);
impl<E: Copy> StopGuard<'_, E> {
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) fn publish(self, exit: E) {
        self.0.result.set(Some(Ok(exit)));
        self.0.waker.wake();
    }
}
impl<E: Copy> Drop for StopGuard<'_, E> {
    fn drop(&mut self) {
        if self.0.result.get().is_none() {
            self.0.result.set(Some(Err(RunnerLost)));
            self.0.waker.wake();
        }
    }
}
/// Read-only product state in the Control/Runner context.
pub struct StatusReader<'a, N: Notifies = SameHart> {
    inner: runtime::StatusReader<'a, Ch<N>>,
}
impl<N: Notifies> Clone for StatusReader<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: Notifies> Copy for StatusReader<'_, N> {}
impl<N: Notifies> StatusReader<'_, N> {
    pub fn status(&self) -> runtime::Status {
        self.inner.status()
    }
}
/// A received buffer the station lends to the network: the S31 port's
/// receive unit on the chip, whose return tells through `P`.
#[cfg(target_arch = "riscv32")]
type LentReceive<P> = oer_esp32s31_ieee80211_driver::port::LentReceive<
    oer_esp32s31_ieee80211_driver::port::device::StagedReceive<{ chip::Q }, { chip::B }>,
    P,
>;
/// The lent buffer of `N`'s station.
#[cfg(target_arch = "riscv32")]
type LentRx<N> = LentReceive<<N as Notifies>::Lent>;
#[cfg(not(target_arch = "riscv32"))]
type LentRx<N> = <N as Notifies>::LentRx;
/// The received buffers the station may lend the network at once: the
/// bound for the network's packet slots of lent frames.
#[cfg(target_arch = "riscv32")]
pub const LENT_RX: usize = oer_esp32s31_ieee80211_driver::port::lent_rx(chip::Q);
/// The host build has no receive buffer to lend.
#[cfg(not(target_arch = "riscv32"))]
pub enum Unlent {}
#[cfg(not(target_arch = "riscv32"))]
impl oer_ieee80211_contracts_port::LentRxBuffer for Unlent {
    fn storage(&self) -> &[u8] {
        match *self {}
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        match *self {}
    }
    fn mpdu_len(&self) -> usize {
        match *self {}
    }
    fn advance(&mut self, _: usize) -> Result<(), oer_ieee80211_contracts_port::AdvanceRefused> {
        match *self {}
    }
}
type Frame<N> = runtime::Received<LentRx<N>>;
type EthernetCopy = oer_ieee80211_core_roles_sta::data::EthernetFrame;

/// Affine neutral Ethernet end. The concrete station frame is private.
pub struct Network<'a, N: Notifies = SameHart> {
    inner: runtime::Device<'a, LentRx<N>, 2, RX_DEPTH, Ch<N>>,
}
/// One received MSDU: in the station's receive buffer, or the mark of a
/// copy [`Network`] keeps.
pub struct Received<N: Notifies = SameHart> {
    inner: Frame<N>,
}
/// What stands for a copied [`Received`] that [`Network`] keeps.
pub struct Copied(runtime::CopiedMark);
/// The receive buffer a lent [`Received`] keeps its frame in. Dropping it
/// returns the buffer to the radio.
pub struct ReceivedStorage<N: Notifies = SameHart>(runtime::NetworkRx<LentRx<N>>);
impl<N: Notifies> oer_network_port::FrameStorage for ReceivedStorage<N> {
    fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        self.0.bytes_mut()
    }
    fn advance(&mut self, by: usize) -> Result<(), oer_network_port::StorageAdvanceRefused> {
        self.0.advance(by)
    }
}
impl<N: Notifies> oer_network_port::ReceivedFrame for Received<N> {
    type Storage = ReceivedStorage<N>;
    type Copied = Copied;
    fn into_lent(self) -> Result<oer_network_port::LentFrame<ReceivedStorage<N>>, Copied> {
        match self.inner {
            runtime::Received::Lent(frame) => Ok(frame.map_storage(ReceivedStorage)),
            runtime::Received::Copied(mark) => Err(Copied(mark)),
        }
    }
}
impl<N: Notifies> NetDevice for Network<'_, N> {
    type RxFrame = Received<N>;
    fn link(&self) -> LinkSnapshot {
        self.inner.link()
    }
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        self.inner.poll_link(cx)
    }
    fn receive_queue_len(&self) -> usize {
        self.inner.receive_queue_len()
    }
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.poll_receive_ready(cx)
    }
    fn try_receive(&mut self) -> Option<Received<N>> {
        self.inner.try_receive().map(|inner| Received { inner })
    }
    fn read_copied<R>(
        &mut self,
        copied: Copied,
        read: impl FnOnce(oer_network_port::EthernetFrameView<'_>) -> R,
    ) -> Result<R, oer_network_port::FrameLengthError> {
        self.inner.read_copied(copied.0, read)
    }
    fn poll_transmit_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.poll_transmit_ready(cx)
    }
    fn try_transmit<F: EthernetFrame>(&mut self, frame: F) -> Result<(), TransmitRefused<F>> {
        self.inner.try_transmit(frame)
    }
}
/// Caller-owned queues, valid before a port installation exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    Unavailable,
}
struct StartState {
    result: core::cell::Cell<Option<Result<(), StartError>>>,
    waker: oer_sync::PriorityWaker,
}
impl StartState {
    const fn new() -> Self {
        Self {
            result: core::cell::Cell::new(None),
            waker: oer_sync::PriorityWaker::new(),
        }
    }
    fn complete(&self, result: Result<(), StartError>) {
        self.result.set(Some(result));
        self.waker.wake();
    }
}
struct StartGuard<'a>(&'a StartState);
impl StartGuard<'_> {
    fn complete(&self, result: Result<(), StartError>) {
        self.0.complete(result);
    }
}
impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        if self.0.result.get().is_none() {
            self.complete(Err(StartError::Unavailable));
        }
    }
}
/// The station's stop request to the network side and its answer. The two
/// sides may run on different harts: the flags are atomic, and each side
/// waits on its own notify (`N`'s wakers).
struct NetworkStopState<N: Notifies> {
    requested: AtomicBool,
    stopped: AtomicBool,
    lost: AtomicBool,
    /// The composition has no network: the station delivers no frame.
    absent: AtomicBool,
    network: <Ch<N> as runtime::Notifies>::NetworkWaker,
    station: <Ch<N> as runtime::Notifies>::StationWaker,
}
impl NetworkStopState<SameHart> {
    const fn new() -> Self {
        Self::with(
            oer_sync::PriorityWaker::new(),
            oer_sync::PriorityWaker::new(),
        )
    }
}
impl<N: Notifies> NetworkStopState<N> {
    const fn with(
        network: <Ch<N> as runtime::Notifies>::NetworkWaker,
        station: <Ch<N> as runtime::Notifies>::StationWaker,
    ) -> Self {
        Self {
            requested: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            absent: AtomicBool::new(false),
            network,
            station,
        }
    }
    fn reset(&self) {
        for flag in [&self.requested, &self.stopped, &self.lost, &self.absent] {
            flag.store(false, Ordering::Release);
        }
    }
}
impl<N: Notifies> runtime::NetworkStop for NetworkStopState<N> {
    fn takes_frames(&self) -> bool {
        !self.absent.load(Ordering::Acquire)
    }
    fn request_stop(&self) {
        self.requested.store(true, Ordering::Release);
        self.network.notify();
    }
    fn poll_stopped(&self, cx: &mut Context<'_>) -> Poll<Result<(), runtime::NetworkStopLost>> {
        self.station.register(cx);
        if self.stopped.load(Ordering::Acquire) {
            Poll::Ready(Ok(()))
        } else if self.lost.load(Ordering::Acquire) {
            Poll::Ready(Err(runtime::NetworkStopLost))
        } else {
            Poll::Pending
        }
    }
}
/// The network side's end of the station's stop: it stops the application
/// and the IP service, which drop every frame they hold, then reports it.
/// Dropping it unreported fails the station's stop as a lost participant.
pub struct NetworkStopper<'a, N: Notifies = SameHart> {
    state: &'a NetworkStopState<N>,
}
impl<N: Notifies> NetworkStopper<'_, N> {
    /// Ready once the station asked the network to stop.
    pub async fn requested(&self) {
        core::future::poll_fn(|cx| {
            self.state.network.register(cx);
            if self.state.requested.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
    /// The application and the IP service stopped and hold no frame of the
    /// station. The station still waits for every lent buffer's return.
    pub fn stopped(self) {
        self.report(|state| &state.stopped);
    }
    /// The composition has no network side: it gives up the station's
    /// network end, the station delivers no frame (each received MSDU's
    /// buffer goes back at once), and its stop needs no answer.
    pub fn no_network(self, network: Network<'_, N>) {
        drop(network);
        self.state.absent.store(true, Ordering::Release);
        self.report(|state| &state.stopped);
    }
    fn report(self, flag: impl FnOnce(&NetworkStopState<N>) -> &AtomicBool) {
        let state = self.state;
        core::mem::forget(self);
        flag(state).store(true, Ordering::Release);
        state.station.notify();
    }
}
impl<N: Notifies> Drop for NetworkStopper<'_, N> {
    fn drop(&mut self) {
        self.state.lost.store(true, Ordering::Release);
        self.state.station.notify();
    }
}
/// The station's channels: its commands, its network's transmit, receive
/// and copies, the network's stop and the link, with `N`'s notifies.
pub struct Channels<N: Notifies = SameHart> {
    network_stop: NetworkStopState<N>,
    start: StartState,
    stop: StopState<Exit>,
    commands: WakingSpscQueue<runtime::ServiceCommand<Caps>, 2>,
    tx: oer_memory::WakingSlotQueue<TxMsdu, 2, Station<N>, Net<N>>,
    rx: WakingSpscQueue<Frame<N>, RX_DEPTH, Net<N>, Station<N>>,
    rx_copied: WakingSpscQueue<EthernetCopy, { runtime::RX_COPIED_DEPTH }, Net<N>, Station<N>>,
    state: runtime::State<Ch<N>>,
}
/// A network-side queue end's notify of `N`.
type Net<N> = <Ch<N> as runtime::Notifies>::Network;
/// A station-side queue end's notify of `N`.
type Station<N> = <Ch<N> as runtime::Notifies>::Station;
impl Channels {
    pub const fn new() -> Self {
        Self {
            network_stop: NetworkStopState::new(),
            start: StartState::new(),
            stop: StopState::new(),
            commands: WakingSpscQueue::new(),
            tx: oer_memory::WakingSlotQueue::new(),
            rx: WakingSpscQueue::new(),
            rx_copied: WakingSpscQueue::new(),
            state: runtime::State::new(),
        }
    }
}
impl<NW, SW, D> Channels<AcrossHarts<NW, SW, D>>
where
    NW: oer_memory::QueueNotify + Copy + 'static,
    SW: oer_memory::QueueNotify + Copy + 'static,
    D: oer_memory::QueueNotify + Copy + 'static,
{
    /// Channels whose network end ([`Network`], [`NetworkStopper`]) runs
    /// on another hart than the station: `network` raises the network
    /// task's executor signal, `station` the station task's. Each wakes
    /// the one task that polls that end.
    pub const fn across_harts(network: NW, station: SW) -> Self {
        Self {
            network_stop: NetworkStopState::with(network, station),
            start: StartState::new(),
            stop: StopState::new(),
            commands: WakingSpscQueue::new(),
            tx: oer_memory::WakingSlotQueue::across_harts(station, network),
            rx: WakingSpscQueue::across_harts(network, station),
            rx_copied: WakingSpscQueue::across_harts(network, station),
            state: runtime::State::across_harts(network),
        }
    }
}
impl<N: Notifies> Channels<N> {
    #[allow(clippy::type_complexity, reason = "the four ends of one split")]
    pub fn split(
        &mut self,
    ) -> Result<
        (
            Control<'_, N>,
            Network<'_, N>,
            NetworkStopper<'_, N>,
            Connection<'_, N>,
        ),
        runtime::ControlCreateError,
    > {
        self.network_stop.reset();
        let (send, commands) = self.commands.split();
        let (tx, transmit) = self.tx.split(TxMsdu::empty);
        let (receive, rx) = self.rx.split();
        let (receive_copied, rx_copied) = self.rx_copied.split();
        let (control, runner) = runtime::Control::new(send, &self.state).map_err(|r| r.reason)?;
        let network = runtime::Device::new(tx, rx, rx_copied, &self.state);
        Ok((
            Control {
                inner: control,
                start: &self.start,
                stop: &self.stop,
            },
            Network { inner: network },
            NetworkStopper {
                state: &self.network_stop,
            },
            Connection {
                service: ServiceConnection {
                    network_stop: &self.network_stop,
                    start: StartGuard(&self.start),
                    commands,
                    transmit,
                    receive,
                    receive_copied,
                    runner,
                },
                stop: StopGuard(&self.stop),
            },
        ))
    }
}
impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}
/// The runner's ends of the channels and its duty to publish the exit.
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub struct Connection<'a, N: Notifies = SameHart> {
    service: ServiceConnection<'a, N>,
    stop: StopGuard<'a, Exit>,
}
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
struct ServiceConnection<'a, N: Notifies> {
    network_stop: &'a NetworkStopState<N>,
    start: StartGuard<'a>,
    commands: oer_memory::WakingSpscReceiver<'a, runtime::ServiceCommand<Caps>, 2>,
    transmit: oer_memory::SlotReceiver<'a, TxMsdu, 2, Station<N>, Net<N>>,
    receive: oer_memory::WakingSpscSender<'a, Frame<N>, RX_DEPTH, Net<N>, Station<N>>,
    receive_copied: oer_memory::WakingSpscSender<
        'a,
        EthernetCopy,
        { runtime::RX_COPIED_DEPTH },
        Net<N>,
        Station<N>,
    >,
    runner: runtime::RunnerControl<'a, Ch<N>>,
}

#[cfg(target_arch = "riscv32")]
mod chip;
#[cfg(target_arch = "riscv32")]
mod dma;
pub mod policy;
mod reconnect;
#[cfg(any(test, target_arch = "riscv32"))]
mod release;
pub use reconnect::{ReconnectPolicy, ZeroReconnectInterval};
#[cfg(target_arch = "riscv32")]
#[doc(hidden)]
pub mod radio;
#[cfg(target_arch = "riscv32")]
#[doc(hidden)]
pub mod station;
#[cfg(target_arch = "riscv32")]
pub use dma::{DmaError, DmaResources};
/// One reading of the S31 receive driver's counters.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_driver::rx::counters::WifiRxReport;

/// The receive driver's counters, once the station's receive path is bound:
/// a trace build's diagnostic reading, never a product value.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) static RX_COUNTERS: oer_memory::OnceValue<
    &'static oer_esp32s31_ieee80211_driver::rx::counters::WifiRxCounters,
> = oer_memory::OnceValue::new();

/// The receive driver's counters now; `None` before the station's receive
/// path was first bound. Only a trace build reads them.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub fn rx_counters() -> Option<WifiRxReport> {
    RX_COUNTERS.get().map(|counters| counters.report())
}
// The facade's own API, not aliases of backend or runtime modules.
#[cfg(target_arch = "riscv32")]
pub use chip::{
    Bell, Binding, Config, Driver, Event, Events, InitializationError, InterruptTypes, Interrupts,
    Lower, Profile, Resources, Runners, StationExit, StationRunner, Storage, UnavailableCause,
    station,
};

/// Bind the facade's backing to this image's concrete interrupt entries.
/// The image declares callbacks, collectors, the composed engine type and terminal policy.
#[cfg(target_arch = "riscv32")]
#[macro_export]
macro_rules! bind_station {
 ($vis:vis mod $name:ident {wifi:$wifi:ty,radio:$radio:ty,callbacks:$callbacks:ty,observer:$observer:ty,driver_observer:$driver_observer:ty,sink:$sink:ty,engine:$engine:ty,events:$events:ty,fail:$fail:path $(,)?})=>{
  $crate::bind_station!(@bind $vis mod $name {wifi:$wifi,radio:$radio,callbacks:$callbacks,observer:$observer,driver_observer:$driver_observer,sink:$sink,engine:$engine,events:$events,fail:$fail}
   notifies: $crate::SameHart, storage: $crate::Storage::new());
 };
 // The network end runs on another hart: `network` raises the network
 // task's signal, `station` the station task's, `driver` the driver's, each
 // a notify of the type it names.
 ($vis:vis mod $name:ident {wifi:$wifi:ty,radio:$radio:ty,callbacks:$callbacks:ty,observer:$observer:ty,driver_observer:$driver_observer:ty,sink:$sink:ty,engine:$engine:ty,events:$events:ty,fail:$fail:path,across_harts:{network:$nt:ty=$network:expr,station:$st:ty=$station:expr,driver:$dt:ty=$driver:expr $(,)?} $(,)?})=>{
  $crate::bind_station!(@bind $vis mod $name {wifi:$wifi,radio:$radio,callbacks:$callbacks,observer:$observer,driver_observer:$driver_observer,sink:$sink,engine:$engine,events:$events,fail:$fail}
   notifies: $crate::AcrossHarts<$nt,$st,$dt>, storage: $crate::Storage::across_harts($network,$station,$driver));
 };
 (@bind $vis:vis mod $name:ident {wifi:$wifi:ty,radio:$radio:ty,callbacks:$callbacks:ty,observer:$observer:ty,driver_observer:$driver_observer:ty,sink:$sink:ty,engine:$engine:ty,events:$events:ty,fail:$fail:path} notifies: $notifies:ty, storage: $storage:expr)=>{
  $vis mod $name {
   use super::*;
   pub struct Binding;
   oer_espressif_executor::declare_port_slot!(pub slot:$crate::radio::Backend<Id,super::Binding>);
   impl $crate::Binding for Binding {
    type Id=slot::Id;type Wifi=$wifi;type Radio=$radio;type Callbacks=$callbacks;type Observer=$observer;type DriverObserver=$driver_observer;type Sink=$sink;type Engine=$engine;type Events=$events;
    type Notifies=$notifies;
    fn take_slot()->Option<&'static mut $crate::radio::Slot<Self>>{slot::take_static()}
    fn fail(error:$crate::InitializationError)->!{$fail(error)}
   }
   pub fn storage()->&'static mut $crate::Storage<Binding>{
    static STORAGE:static_cell::ConstStaticCell<$crate::Storage<Binding>>=static_cell::ConstStaticCell::new($storage);
    STORAGE.try_take().unwrap_or_else(||<Binding as $crate::Binding>::fail($crate::InitializationError::Storage))
   }
  }
 };
}

#[cfg(target_arch = "riscv32")]
mod actor;
#[cfg(target_arch = "riscv32")]
pub use actor::{RadioActor, RadioResources, WifiHardware};

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]
mod tests {
    extern crate std;
    use self::std::{cell::Cell, rc::Rc};
    use super::*;
    struct Packet(Rc<Cell<usize>>);
    impl Drop for Packet {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    impl EthernetFrame for Packet {
        fn view(
            &self,
        ) -> Result<oer_network_port::EthernetFrameView<'_>, oer_network_port::FrameLengthError>
        {
            Ok(oer_network_port::EthernetFrameView {
                destination: [2; 6],
                source: [4; 6],
                ether_type: 0x0800,
                payload: &[7, 8],
            })
        }
    }
    #[test]
    fn a_stopper_reports_its_stop_and_a_dropped_one_is_a_lost_participant() {
        use runtime::NetworkStop as _;
        let mut cx = Context::from_waker(core::task::Waker::noop());
        let state = NetworkStopState::new();
        state.request_stop();
        let stopper = NetworkStopper { state: &state };
        assert!(state.poll_stopped(&mut cx).is_pending());
        stopper.stopped();
        assert_eq!(state.poll_stopped(&mut cx), Poll::Ready(Ok(())));
        state.reset();
        let mut channels = Channels::new();
        let (_, network, _, _) = channels.split().unwrap();
        assert!(state.takes_frames());
        NetworkStopper { state: &state }.no_network(network);
        assert_eq!(state.poll_stopped(&mut cx), Poll::Ready(Ok(())));
        assert!(!state.takes_frames(), "no network takes no frame");
        state.reset();
        // Losing the stopper proves no stop of the application.
        drop(NetworkStopper { state: &state });
        assert_eq!(
            state.poll_stopped(&mut cx),
            Poll::Ready(Err(runtime::NetworkStopLost))
        );
    }
    #[test]
    fn control_and_neutral_device_exist_before_installation_without_minting_a_link_or_consuming_refused_owners()
     {
        let mut channels = Channels::new();
        let (mut control, mut net, _stopper, connection) = channels.split().unwrap();
        assert_eq!(control.link(), LinkSnapshot::DOWN);
        assert_eq!(net.link(), LinkSnapshot::DOWN);
        let dropped = Rc::new(Cell::new(0));
        let refused = net.try_transmit(Packet(dropped.clone())).err().unwrap();
        assert_eq!(refused.reason, oer_network_port::TransmitError::LinkDown);
        assert!(Rc::ptr_eq(&refused.frame.0, &dropped));
        assert_eq!(dropped.get(), 0);
        let credentials =
            embassy_futures::block_on(crate::StationSettings::open(b"lab").unwrap().prepare())
                .unwrap();
        let _connect = embassy_futures::block_on(control.connect(credentials)).unwrap();
        let _disconnect = embassy_futures::block_on(control.disconnect()).unwrap();
        {
            // Both command cells are taken: the scan waits for room.
            let mut scan = core::pin::pin!(control.scan());
            assert!(
                core::future::Future::poll(
                    scan.as_mut(),
                    &mut Context::from_waker(core::task::Waker::noop())
                )
                .is_pending()
            );
        }
        let crate::Connection {
            service:
                crate::ServiceConnection {
                    network_stop: _,
                    start,
                    commands,
                    transmit: _,
                    receive: _,
                    receive_copied: _,
                    runner,
                },
            stop,
        } = connection;
        assert!(matches!(
            commands.try_receive(),
            Some(runtime::ServiceCommand::Station(r)) if r.command() == runtime::contract::Command::Connect
        ));
        assert!(matches!(
            commands.try_receive(),
            Some(runtime::ServiceCommand::Station(r)) if r.command() == runtime::contract::Command::Disconnect
        ));
        assert!(commands.try_receive().is_none());
        start.complete(Ok(()));
        assert_eq!(embassy_futures::block_on(control.wait_started()), Ok(()));
        assert_eq!(net.link(), LinkSnapshot::DOWN);
        // Losing the uninstalled runner poisons its control connection, not a new
        // backend installation or a replacement command producer.
        drop(runner);
        assert_eq!(
            embassy_futures::block_on(control.scan()).err(),
            Some(ControlError::Stopped)
        );
        // A runner that never published its exit reports its loss.
        drop(stop);
        assert_eq!(
            embassy_futures::block_on(control.wait_stopped()),
            Err(RunnerLost)
        );
        drop(refused.frame);
        assert_eq!(dropped.get(), 1);
    }
    #[test]
    fn the_published_exit_is_a_copy_waiters_read_once_it_exists() {
        let state = StopState::<u8>::new();
        let mut cx = Context::from_waker(core::task::Waker::noop());
        {
            let mut waiting = core::pin::pin!(state.wait());
            assert!(core::future::Future::poll(waiting.as_mut(), &mut cx).is_pending());
        }
        StopGuard(&state).publish(7);
        assert_eq!(embassy_futures::block_on(state.wait()), Ok(7));
        assert_eq!(embassy_futures::block_on(state.wait()), Ok(7));
    }
    #[test]
    fn start_wait_is_cancellation_safe_and_a_dropped_uninstalled_runner_wakes_it_with_a_typed_failure()
     {
        use self::std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            task::Wake,
        };
        use core::future::Future;
        struct Wakes(AtomicUsize);
        impl Wake for Wakes {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = core::task::Waker::from(wakes.clone());
        let mut channels = Channels::new();
        let (mut control, _network, _stopper, connection) = channels.split().unwrap();
        let mut cx = Context::from_waker(&waker);
        {
            let mut waiting = core::pin::pin!(control.wait_started());
            assert!(waiting.as_mut().poll(&mut cx).is_pending());
        }
        let mut waiting = core::pin::pin!(control.wait_started());
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        drop(connection);
        assert!(wakes.0.load(Ordering::Relaxed) > 0);
        assert_eq!(
            waiting.as_mut().poll(&mut cx),
            Poll::Ready(Err(StartError::Unavailable))
        );
    }
}
