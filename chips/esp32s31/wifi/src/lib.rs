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
use core::task::{Context, Poll};
pub use credentials::{
    Callbacks, CredentialsError, NonceExhausted, NonceSource, OpenCallbacks, PreparedCredentials,
    StationSettings,
};
use oer_ieee80211_runtime::station as runtime;
use oer_ieee80211_sta::data::{EthernetFrame as Frame, TxMsdu};
use oer_memory::WakingSpscQueue;
use oer_network::{EthernetFrame, LinkSnapshot, NetDevice, TransmitRefused};
use runtime::contract::Command;

pub const RX_DEPTH: usize = 8;
pub const FRAME_CAPACITY: usize = 1664;
#[cfg(target_arch = "riscv32")]
type Caps = oer_esp32s31_ieee80211_driver::port::Esp32s31Caps<FRAME_CAPACITY>;
#[cfg(not(target_arch = "riscv32"))]
type Caps = ();

/// The sole product command end. Its type carries no port identity or loan.
pub struct Control<'a> {
    start: &'a StartState,
    inner: runtime::Control<'a, Caps, 2>,
}
impl<'a> Control<'a> {
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
    pub fn status_reader(&self) -> runtime::StatusReader<'a> {
        self.inner.status_reader()
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
    pub fn try_connect(&mut self, correlation: u64) -> Result<(), runtime::ControlError> {
        self.request(correlation, Command::Connect)
    }
    pub fn try_disconnect(&mut self, correlation: u64) -> Result<(), runtime::ControlError> {
        self.request(correlation, Command::Disconnect)
    }
    pub fn try_scan(&mut self, correlation: u64) -> Result<(), runtime::ControlError> {
        self.request(correlation, Command::BackgroundScan)
    }
    fn request(&mut self, correlation: u64, command: Command) -> Result<(), runtime::ControlError> {
        self.inner
            .try_command(runtime::ServiceCommand::Station(runtime::Request::new(
                correlation,
                command,
            )))
            .map_err(|r| r.reason)
    }
    pub async fn connect(&mut self, correlation: u64) -> Result<(), runtime::ControlError> {
        self.inner.connect(correlation).await.map_err(|r| r.reason)
    }
    pub async fn disconnect(&mut self, correlation: u64) -> Result<(), runtime::ControlError> {
        self.inner
            .disconnect(correlation)
            .await
            .map_err(|r| r.reason)
    }
    pub fn stop(&mut self) {
        self.inner.stop();
    }
    pub async fn wait_stopped(&mut self) {
        self.inner.wait_stopped().await;
    }
}
/// Affine neutral Ethernet end. The concrete station frame is private.
pub struct Network<'a> {
    inner: runtime::Device<'a, 2, RX_DEPTH>,
}
pub struct Received {
    inner: Frame,
}
impl EthernetFrame for Received {
    fn view(&self) -> Result<oer_network::EthernetFrameView<'_>, oer_network::FrameLengthError> {
        self.inner.view()
    }
}
impl NetDevice for Network<'_> {
    type RxFrame = Received;
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
    fn try_receive(&mut self) -> Option<Received> {
        self.inner.try_receive().map(|inner| Received { inner })
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
pub struct Channels {
    start: StartState,
    commands: WakingSpscQueue<runtime::ServiceCommand<Caps>, 2>,
    tx: WakingSpscQueue<TxMsdu, 2>,
    rx: WakingSpscQueue<Frame, RX_DEPTH>,
    state: runtime::State,
}
impl Channels {
    pub const fn new() -> Self {
        Self {
            start: StartState::new(),
            commands: WakingSpscQueue::new(),
            tx: WakingSpscQueue::new(),
            rx: WakingSpscQueue::new(),
            state: runtime::State::new(),
        }
    }
    pub fn split(
        &mut self,
    ) -> Result<
        (Control<'_>, Network<'_>, Connection<'_>),
        oer_time::connected_tasks::ConnectedTaskControlError,
    > {
        let (send, commands) = self.commands.split();
        let (tx, transmit) = self.tx.split();
        let (receive, rx) = self.rx.split();
        let (control, runner) = runtime::Control::new(send, &self.state).map_err(|r| r.reason)?;
        let network = runtime::Device::new(tx, rx, &self.state);
        Ok((
            Control {
                inner: control,
                start: &self.start,
            },
            Network { inner: network },
            Connection {
                start: StartGuard(&self.start),
                commands,
                transmit,
                receive,
                runner,
            },
        ))
    }
}
impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub struct Connection<'a> {
    start: StartGuard<'a>,
    commands: oer_memory::WakingSpscReceiver<'a, runtime::ServiceCommand<Caps>, 2>,
    transmit: oer_memory::WakingSpscReceiver<'a, TxMsdu, 2>,
    receive: oer_memory::WakingSpscSender<'a, Frame, RX_DEPTH>,
    runner: runtime::RunnerControl<'a>,
}

#[cfg(target_arch = "riscv32")]
mod chip;
pub mod policy;
#[cfg(target_arch = "riscv32")]
#[doc(hidden)]
pub mod radio;
#[cfg(target_arch = "riscv32")]
#[doc(hidden)]
pub mod station;
// The facade's own API, not aliases of backend or runtime modules.
#[cfg(target_arch = "riscv32")]
pub use chip::{
    Bell, Binding, Config, DmaTypes, Driver, Event, Events, InitializationError, InterruptTypes,
    Interrupts, Lower, Profile, Resources, Runners, StationExit, StationRunner, Storage,
    UnavailableCause, station,
};

/// Bind the facade's backing to this image's concrete interrupt entries.
/// The image chooses its callbacks, collectors, engine and terminal policy.
#[cfg(target_arch = "riscv32")]
#[macro_export]
macro_rules! bind_station {
 ($vis:vis mod $name:ident {wifi:$wifi:ty,radio:$radio:ty,callbacks:$callbacks:ty,observer:$observer:ty,driver_observer:$driver_observer:ty,engine:$engine:ty,events:$events:ty,fail:$fail:path $(,)?})=>{
  $vis mod $name {
   use super::*;
   pub struct Binding;
   oer_espressif_executor::declare_port_slot!(pub slot:$crate::radio::Backend<Id,super::Binding>);
   impl $crate::Binding for Binding {
    type Id=slot::Id;type Wifi=$wifi;type Radio=$radio;type Callbacks=$callbacks;type Observer=$observer;type DriverObserver=$driver_observer;type Engine=$engine;type Events=$events;
    fn take_slot()->Option<&'static mut $crate::radio::Slot<Self>>{slot::take_static()}
    fn fail(error:$crate::InitializationError)->!{$fail(error)}
   }
   pub fn storage()->&'static mut $crate::Storage<Binding>{
    static STORAGE:static_cell::ConstStaticCell<$crate::Storage<Binding>>=static_cell::ConstStaticCell::new($crate::Storage::new());
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
        ) -> Result<oer_network::EthernetFrameView<'_>, oer_network::FrameLengthError> {
            Ok(oer_network::EthernetFrameView {
                destination: [2; 6],
                source: [4; 6],
                ether_type: 0x0800,
                payload: &[7, 8],
            })
        }
    }
    #[test]
    fn control_and_neutral_device_exist_before_installation_without_minting_a_link_or_consuming_refused_owners()
     {
        let mut channels = Channels::new();
        let (mut control, mut net, connection) = channels.split().unwrap();
        assert_eq!(control.link(), LinkSnapshot::DOWN);
        assert_eq!(net.link(), LinkSnapshot::DOWN);
        let dropped = Rc::new(Cell::new(0));
        let refused = net.try_transmit(Packet(dropped.clone())).err().unwrap();
        assert_eq!(refused.reason, oer_network::TransmitError::LinkDown);
        assert!(Rc::ptr_eq(&refused.frame.0, &dropped));
        assert_eq!(dropped.get(), 0);
        assert_eq!(control.try_connect(41), Ok(()));
        assert_eq!(control.try_disconnect(42), Ok(()));
        assert_eq!(control.try_scan(43), Err(runtime::ControlError::QueueFull));
        let crate::Connection {
            start,
            commands,
            transmit: _,
            receive: _,
            runner,
        } = connection;
        assert!(
            matches!(commands.try_receive(),Some(runtime::ServiceCommand::Station(r)) if r.correlation()==41)
        );
        assert!(
            matches!(commands.try_receive(),Some(runtime::ServiceCommand::Station(r)) if r.correlation()==42)
        );
        assert!(commands.try_receive().is_none());
        start.complete(Ok(()));
        assert_eq!(embassy_futures::block_on(control.wait_started()), Ok(()));
        assert_eq!(net.link(), LinkSnapshot::DOWN);
        // Losing the uninstalled runner poisons its control connection, not a new
        // backend installation or a replacement command producer.
        drop(runner);
        assert_eq!(control.try_connect(44), Err(runtime::ControlError::Stopped));
        drop(refused.frame);
        assert_eq!(dropped.get(), 1);
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
        let (mut control, _network, connection) = channels.split().unwrap();
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
