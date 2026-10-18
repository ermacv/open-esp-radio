use super::*;
use core::task::{Context, Poll, Waker};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_embassy_net_owned::OwnedEndpointResources;
use std::boxed::Box;
use xarxa_driver::{PacketPool, PacketPoolStorage};

struct Services<'a> {
    completion: &'a Signal<NoopRawMutex, ()>,
    prepared: Option<Box<[u8; 14]>>,
    completions: usize,
    fail_completion: bool,
    stops: usize,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for Services<'_>
{
    type Error = ();
    type Exit = ();

    async fn service_rx(
        &mut self,
        _: &mut dyn DatapathNetworkRxSet,
        _: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, ()> {
        Ok(DatapathRxProgress::Drained)
    }

    async fn start_tx<'a, I>(&'a mut self, _: S, _: &'a I) -> Result<WifiTxProgress, ()>
    where
        S: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P> + 'a,
    {
        panic!("a requested boundary must not admit TX")
    }

    async fn wait_tx_deadline(&mut self) {
        self.completion.wait().await;
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        self.completions += 1;
        if self.fail_completion {
            Err(())
        } else {
            Ok(WifiTxProgress::Complete)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.prepared.is_some()
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        assert_eq!(*self.prepared.take().expect("retained frame"), [0x42; 14]);
        self.completion.signal(());
        Ok(WifiTxProgress::Complete)
    }

    fn cancel_prepared_tx<I>(&mut self, _: &I) -> Result<(), ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        self.prepared.take();
        Ok(())
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, ()> {
        self.stops += 1;
        Ok(DatapathStopProgress::Stopped)
    }
}

fn exercise_stop(active: bool, fail_completion: bool) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        Services {
            completion: &completion,
            prepared: Some(Box::new([0x42; 14])),
            completions: 0,
            fail_completion,
            stops: 0,
        },
        oer_time_virtual::SkipClock::new(),
    );
    runner.prepared_tx_interface = Some(interface);
    if active {
        runner.begin_active_tx(interface, DatapathTxOrigin::Network);
    }
    let control = execution::Control::<NoopRawMutex>::new();
    control.request_stop();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut run = core::pin::pin!(runner.run_controlled(&control));
        if active {
            assert!(run.as_mut().poll(&mut cx).is_pending());
            // Repoll without an event cannot complete the active owner.
            assert!(
                run.as_mut().poll(&mut cx).is_pending(),
                "stop cannot drop in-flight TX"
            );
            completion.signal(());
        }
        let expected = if fail_completion {
            Err(())
        } else {
            Ok(DatapathRunnerExit::Stopped)
        };
        assert_eq!(run.as_mut().poll(&mut cx), Poll::Ready(expected));
    }
    assert_eq!(runner.services.completions, usize::from(active));
    if fail_completion {
        // The failed completion retains the live transaction and the link;
        // the latched stop finishes it on the next run of the same owner.
        assert_eq!(runner.active_tx_interface, Some(interface));
        assert!(device.link_is_up());
        runner.services.fail_completion = false;
        completion.signal(());
        let mut run = core::pin::pin!(runner.run_controlled(&control));
        assert_eq!(
            run.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathRunnerExit::Stopped))
        );
    }
    assert!(control.stop_requested());
    assert!(!device.link_is_up());
    assert!(runner.services.prepared.is_none());
    assert_eq!(runner.services.stops, 1);
}

#[test]
fn stop_cancels_prepared_tx_and_takes_the_link_down() {
    exercise_stop(false, false);
}

#[test]
fn stop_waits_for_active_tx_event_before_returning() {
    exercise_stop(true, false);
}

#[test]
fn failed_tx_drain_retains_the_transaction_and_the_latched_stop() {
    exercise_stop(true, true);
}

#[test]
fn control_exchange_waits_for_each_terminal_event_without_admitting_prepared_data() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        Services {
            completion: &completion,
            prepared: Some(Box::new([0x42; 14])),
            completions: 0,
            fail_completion: false,
            stops: 0,
        },
        oer_time_virtual::SkipClock::new(),
    );
    runner.prepared_tx_interface = Some(interface);

    let original = runner.services.prepared.as_ref().unwrap().as_ptr();
    let mut step = 0;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut exchange = core::pin::pin!(runner.run_control_exchange(|services| {
            assert_eq!(services.completions, step);
            step += 1;
            Ok(if step <= 2 {
                DatapathControlProgress::TxPending
            } else {
                DatapathControlProgress::Idle
            })
        }));
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        completion.signal(());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
        completion.signal(());
        assert_eq!(exchange.as_mut().poll(&mut cx), Poll::Ready(Ok(None)));
    }
    assert_eq!(step, 3);
    assert_eq!(runner.services.completions, 2);
    assert_eq!(runner.services.stops, 0);
    assert_eq!(
        runner.services.prepared.as_ref().unwrap().as_ptr(),
        original
    );
    assert_eq!(runner.prepared_tx_interface, Some(interface));
    assert!(runner.active_tx_interface.is_none());
    let _ = device;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChainEvent {
    StartNetwork,
    StartPrepared(u8),
    Completed,
    Control,
    Rx,
}

/// A saturated role: every completion leaves a one-MPDU successor until the
/// configured count is exhausted.
struct ChainServices<'a> {
    completion: &'a Signal<NoopRawMutex, ()>,
    log: std::vec::Vec<ChainEvent>,
    successors: u8,
    prepared: Option<u8>,
    control_after_successor: u8,
    control_pending: bool,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for ChainServices<'_>
{
    type Error = ();
    type Exit = ();

    async fn service_rx(
        &mut self,
        _: &mut dyn DatapathNetworkRxSet,
        _: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, ()> {
        self.log.push(ChainEvent::Rx);
        Ok(DatapathRxProgress::Drained)
    }

    async fn start_tx<'a, I>(&'a mut self, _: S, _: &'a I) -> Result<WifiTxProgress, ()>
    where
        S: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P> + 'a,
    {
        self.log.push(ChainEvent::StartNetwork);
        Ok(WifiTxProgress::Pending)
    }

    async fn wait_tx_deadline(&mut self) {
        self.completion.wait().await;
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        self.log.push(ChainEvent::Completed);
        let published = self
            .log
            .iter()
            .filter(|event| matches!(event, ChainEvent::StartPrepared(_)))
            .count() as u8;
        if published < self.successors {
            self.prepared = Some(published + 1);
        }
        if published == self.control_after_successor {
            self.control_pending = true;
        }
        Ok(WifiTxProgress::Complete)
    }

    fn control_ready(&self, _: oer_time::Instant) -> bool {
        self.control_pending
    }

    async fn service_control(
        &mut self,
        _: DatapathControlContext,
    ) -> Result<DatapathControlProgress<()>, ()> {
        self.control_pending = false;
        self.log.push(ChainEvent::Control);
        Ok(DatapathControlProgress::Idle)
    }

    fn has_prepared_tx(&self) -> bool {
        self.prepared.is_some()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        usize::from(self.prepared.is_some())
    }

    /// A 32-MPDU Block Ack window: every successor is far below its target
    /// and must still be published without waiting for more owners.
    fn tx_batch_demand<I>(&self, _: NetworkInterfaceId, _: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        TxBatchDemand {
            target: 32,
            ready: usize::from(self.prepared.is_some()),
        }
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        let successor = self.prepared.take().expect("a complete successor");
        self.log.push(ChainEvent::StartPrepared(successor));
        Ok(WifiTxProgress::Pending)
    }
}

#[test]
fn partial_successors_publish_without_waiting_but_yield_to_ready_control() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let mut frame = allocator.try_alloc().unwrap();
    frame.set_len(15);
    frame.fill(0);
    frame[..6].fill(4);
    device.transmit(frame).unwrap();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let finished = Signal::<NoopRawMutex, ()>::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        ChainServices {
            completion: &completion,
            log: std::vec::Vec::new(),
            successors: 3,
            prepared: None,
            control_after_successor: 1,
            control_pending: false,
        },
        oer_time_virtual::SkipClock::new(),
    );
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut run = core::pin::pin!(runner.run_until(finished.wait()));
        // Four exchanges: the network frame and three successors.
        for _ in 0..4 {
            assert!(run.as_mut().poll(&mut cx).is_pending());
            completion.signal(());
        }
        finished.signal(());
        assert_eq!(
            run.as_mut().poll(&mut cx),
            Poll::Ready(Ok(DatapathRunnerExit::Stopped))
        );
    }
    assert_eq!(
        runner.services.log,
        [
            // A fresh runner services control once before admitting data.
            ChainEvent::Control,
            ChainEvent::StartNetwork,
            ChainEvent::Completed,
            ChainEvent::StartPrepared(1),
            ChainEvent::Completed,
            ChainEvent::Control,
            ChainEvent::StartPrepared(2),
            ChainEvent::Completed,
            ChainEvent::StartPrepared(3),
            ChainEvent::Completed,
        ]
    );
}

#[test]
fn saturated_successors_yield_to_a_received_frame_within_one_transaction() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let mut frame = allocator.try_alloc().unwrap();
    frame.set_len(15);
    frame.fill(0);
    frame[..6].fill(4);
    device.transmit(frame).unwrap();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let completion = Signal::new();
    let finished = Signal::<NoopRawMutex, ()>::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        ChainServices {
            completion: &completion,
            log: std::vec::Vec::new(),
            successors: 12,
            prepared: None,
            control_after_successor: u8::MAX,
            control_pending: false,
        },
        oer_time_virtual::SkipClock::new(),
    );
    let mut cx = Context::from_waker(Waker::noop());
    let received = |log: &[ChainEvent]| log.contains(&ChainEvent::Rx);
    {
        let mut run = core::pin::pin!(runner.run_until(finished.wait()));
        for exchange in 0..12 {
            assert!(run.as_mut().poll(&mut cx).is_pending());
            // A beacon arrives while the third transaction is on air.
            if exchange == 2 {
                irq.notify_rx_handoff();
            }
            completion.signal(());
        }
        finished.signal(());
        assert!(run.as_mut().poll(&mut cx).is_ready());
    }
    let log = &runner.services.log;
    // The frame arrives while the second successor is on air.
    let arrival = log
        .iter()
        .position(|event| *event == ChainEvent::StartPrepared(2))
        .unwrap();
    let rx = log[arrival..]
        .iter()
        .position(|event| *event == ChainEvent::Rx)
        .map(|offset| arrival + offset);
    assert!(
        received(&log[arrival..]),
        "a saturated TX chain never serviced RX: {log:?}"
    );
    let starts_before_rx = log[arrival + 1..rx.unwrap()]
        .iter()
        .filter(|event| matches!(event, ChainEvent::StartPrepared(_)))
        .count();
    assert!(
        starts_before_rx <= 1,
        "RX waited {starts_before_rx} transactions behind saturated TX: {log:?}"
    );
}

/// Control that is always ready and never consumes its input.
struct SpinningControl {
    steps: u32,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for SpinningControl
{
    type Error = ();
    type Exit = ();

    async fn service_rx(
        &mut self,
        _: &mut dyn DatapathNetworkRxSet,
        _: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, ()> {
        Ok(DatapathRxProgress::Drained)
    }

    async fn start_tx<'a, I>(&'a mut self, _: S, _: &'a I) -> Result<WifiTxProgress, ()>
    where
        S: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P> + 'a,
    {
        panic!("spinning control admits no network TX")
    }

    async fn wait_tx_deadline(&mut self) {
        core::future::pending().await
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        Ok(WifiTxProgress::Complete)
    }

    fn control_ready(&self, _: oer_time::Instant) -> bool {
        true
    }

    async fn service_control(
        &mut self,
        _: DatapathControlContext,
    ) -> Result<DatapathControlProgress<()>, ()> {
        self.steps += 1;
        Ok(DatapathControlProgress::More)
    }

    fn has_prepared_tx(&self) -> bool {
        false
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        panic!("spinning control prepares no TX")
    }

    fn cancel_prepared_tx<I>(&mut self, _: &I) -> Result<(), ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        Ok(())
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, ()> {
        Ok(DatapathStopProgress::Stopped)
    }
}

#[test]
fn control_that_never_consumes_its_input_cannot_starve_a_sibling_task() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (_device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        SpinningControl { steps: 0 },
        oer_time_virtual::SkipClock::new(),
    );
    let sibling_ran = core::cell::Cell::new(false);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let run = runner.run();
        let sibling = async {
            sibling_ran.set(true);
        };
        let mut both = core::pin::pin!(select(run, sibling));
        assert!(matches!(
            both.as_mut().poll(&mut cx),
            Poll::Ready(Either::Second(()))
        ));
    }
    assert!(sibling_ran.get());
    assert_eq!(runner.services.steps, CONTROL_PROGRESS_BUDGET);
}

/// A dozing station: it holds network frames and must see each one once, to
/// offer it to power management.
struct DozingStation<'a> {
    offered: bool,
    control_steps: &'a core::cell::Cell<u32>,
}

impl<S: SoftwareTxFrame + 'static, P: MaterializedTxFrame + 'static> DatapathServices<S, P>
    for DozingStation<'_>
{
    type Error = ();
    type Exit = ();

    async fn service_rx(
        &mut self,
        _: &mut dyn DatapathNetworkRxSet,
        _: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, ()> {
        Ok(DatapathRxProgress::Drained)
    }

    async fn start_tx<'a, I>(&'a mut self, _: S, _: &'a I) -> Result<WifiTxProgress, ()>
    where
        S: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P> + 'a,
    {
        panic!("a dozing station publishes no held frame")
    }

    async fn wait_tx_deadline(&mut self) {
        core::future::pending().await
    }

    async fn service_tx(&mut self, _: WifiTxWake) -> Result<WifiTxProgress, ()> {
        Ok(WifiTxProgress::Complete)
    }

    fn control_admits_network_tx(&self) -> bool {
        false
    }

    fn control_required_before_network_tx(&self) -> bool {
        !self.offered
    }

    async fn service_control(
        &mut self,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<()>, ()> {
        self.control_steps.set(self.control_steps.get() + 1);
        if context.network_tx_pending {
            self.offered = true;
        }
        Ok(DatapathControlProgress::Idle)
    }

    fn has_prepared_tx(&self) -> bool {
        false
    }

    fn start_prepared_tx<I>(&mut self, _: &I) -> Result<WifiTxProgress, ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        panic!("a dozing station prepares no TX")
    }

    fn cancel_prepared_tx<I>(&mut self, _: &I) -> Result<(), ()>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = S, PhysicalFrame = P>,
    {
        Ok(())
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, ()> {
        Ok(DatapathStopProgress::Stopped)
    }
}

#[test]
fn a_frame_queued_while_the_station_dozes_wakes_control_once() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let allocator = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator);
    let resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = owned::OwnedDatapathNetwork::new(owned, resources.split(pool));
    network.set_link_state(interface, LinkState::Up);
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let control_steps = core::cell::Cell::new(0);
    let mut runner = DatapathRunner::new(
        &irq,
        network,
        interface,
        DozingStation {
            offered: false,
            control_steps: &control_steps,
        },
        oer_time_virtual::SkipClock::new(),
    );
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut run = core::pin::pin!(runner.run());
        assert!(run.as_mut().poll(&mut cx).is_pending());
        let idle_steps = control_steps.get();
        let mut frame = allocator.try_alloc().unwrap();
        frame.set_len(15);
        frame.fill(0);
        frame[..6].fill(4);
        device.transmit(frame).unwrap();
        // The queued frame reaches control once; the held frame then no
        // longer wakes the loop.
        assert!(run.as_mut().poll(&mut cx).is_pending());
        assert!(run.as_mut().poll(&mut cx).is_pending());
        assert_eq!(control_steps.get(), idle_steps + 1);
    }
    assert!(runner.services.offered);
}
