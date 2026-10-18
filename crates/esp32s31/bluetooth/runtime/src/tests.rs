use core::cell::RefCell;
use std::{boxed::Box, rc::Rc, vec, vec::Vec};

use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_radio::{
    AcceptListChange, AcceptListDevice, AdvertisingChannel, AdvertisingChannels,
    AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu, AdvertisingReception,
    AdvertisingSetId, ClockError, CoexistenceLevel, EventId, EventResult, LeInstant, LePhy,
    LeWindow, Poisoned, RadioDuration, RadioOutcome, RadioRequest, ReceivedPdu, RequestError,
    ScanFilterPolicy, ScanType, ScannerConfiguration, ScannerId, Supported, TestChannel, TestPhy,
    TestReceive, TxPower,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequest,
        ControllerTimeRequestError,
    },
    interrupt::{SchedulerWakeBatch, SchedulerWakeCell, SchedulerWorkerWakeClass},
    scheduler::{
        SchedulerFinishedListCaptureError, SchedulerFinishedListWorkerStep, SchedulerHardwareError,
        SchedulerHardwareView, SchedulerIdleInsertion, SchedulerObservation,
        SchedulerSoftwareConfig, SchedulerStartError, SchedulerStep, SchedulerTransactionFault,
        SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY, ControllerSramLinkAddress, LeDeviceTablePublication,
    LeRxChain, SchedulerItemSpace,
};
use oer_esp32s31_bluetooth_radio::validation::model_memory;
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale,
    BluetoothSchedulerBusyObservation, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    BluetoothSchedulerStopped,
};
use oer_time::Clock as _;
use oer_time_virtual::VirtualClock;

use crate::{
    BluetoothFault, BluetoothInstallError, BluetoothOutcome, BluetoothRadioHardware,
    BluetoothRuntime, RxChainPublicationError,
};

#[derive(Default)]
struct State {
    chains_published: bool,
    refuse_chains: bool,
    time: u32,
    started: Vec<ControllerSramLinkAddress>,
    stops: usize,
    /// The stop sequence stays pending.
    stop_never_settles: bool,
    defer_execution: bool,
    running: bool,
    finished: Option<BluetoothSchedulerFinishedListObservation>,
    capture_on_start: bool,
    wake: SchedulerWakeCell,
    routes_disabled: usize,
    routes_restored: usize,
    refuse_start: bool,
    scan_starts: usize,
    device_tables: Vec<LeDeviceTablePublication>,
    /// Controller-time latch requests.
    time_requests: usize,
}

/// The model's owner of a disabled BLE PHY ETM route.
#[must_use]
struct ModelRoute;

/// Hardware that executes the head item at once when it starts.
#[derive(Clone, Default)]
struct Model(Rc<RefCell<State>>);

impl BluetoothRadioHardware for Model {
    type StartError = ();
    type DisabledPhyRoute = ModelRoute;

    fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        SchedulerSoftwareConfig::reviewed_standalone()
    }

    fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
    }

    fn local_sleep_clock_ppm(&self) -> u16 {
        500
    }

    fn request_time(&mut self) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
        self.0.borrow_mut().time_requests += 1;
        Ok(ControllerTimeRequest::for_validation(1))
    }

    fn recheck_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        Ok(ControllerTimeEventStep::Sample {
            request,
            sample: ControllerTimeSample::for_validation(self.0.borrow().time),
        })
    }

    fn drain_time(&mut self) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        Ok(ControllerTimeEventStep::Idle)
    }

    fn publish_rx_chains<const SCANNING: usize, const NON_SCANNING: usize>(
        &mut self,
        _scanning: &LeRxChain<SCANNING>,
        _non_scanning: &LeRxChain<NON_SCANNING>,
    ) -> Result<(), RxChainPublicationError> {
        let mut state = self.0.borrow_mut();
        if state.refuse_chains {
            return Err(RxChainPublicationError::SchedulerStarted);
        }
        state.chains_published = true;
        Ok(())
    }

    fn publish_scan_start(&mut self) {
        self.0.borrow_mut().scan_starts += 1;
    }

    fn publish_device_table(&mut self, publication: LeDeviceTablePublication) {
        self.0.borrow_mut().device_tables.push(publication);
    }

    fn take_wake(&mut self) -> Option<SchedulerWakeBatch> {
        self.0.borrow().wake.take()
    }

    fn capture_finished_lists(
        &mut self,
        _wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        if self.0.borrow().finished.is_some() {
            return Err(SchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.0.borrow_mut().finished =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
        Ok(())
    }

    fn capture_stopped_finished_lists(
        &mut self,
        _stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        if self.0.borrow().finished.is_some() {
            return Err(SchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.0.borrow_mut().finished =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[]);
        Ok(())
    }

    fn next_finished_list(&mut self) -> SchedulerFinishedListWorkerStep {
        let Some(observation) = self.0.borrow_mut().finished.take() else {
            return SchedulerFinishedListWorkerStep::Idle;
        };
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::Complete => {
                SchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => {
                let more = !remaining.is_empty();
                if more {
                    self.0.borrow_mut().finished = Some(remaining);
                }
                SchedulerFinishedListWorkerStep::List { observed, more }
            }
        }
    }

    fn observe(&mut self) -> Result<SchedulerHardwareView, SchedulerHardwareError> {
        // A started item whose execution is deferred keeps the scheduler busy.
        let busy = self.0.borrow().running;
        Ok(SchedulerHardwareView {
            busy: BluetoothSchedulerBusyObservation::from_busy_for_validation(busy),
            hardware_head: None,
        })
    }

    fn disable_phy_etm_route(&mut self) -> ModelRoute {
        self.0.borrow_mut().routes_disabled += 1;
        ModelRoute
    }

    fn restore_phy_etm_route(&mut self, route: ModelRoute) {
        let ModelRoute = route;
        self.0.borrow_mut().routes_restored += 1;
    }

    fn start(
        &mut self,
        items: &SchedulerItemSpace<'_>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<()>> {
        let id = items
            .listed_item(insertion.head)
            .ok_or(SchedulerStartError::ForeignItem(insertion.head))?;
        let mut state = self.0.borrow_mut();
        if state.refuse_start {
            return Err(SchedulerStartError::Interrupts(()));
        }
        state.started.push(insertion.head);
        if state.defer_execution {
            state.running = true;
        } else {
            items.record_status_for_validation(id, 0);
            if state.capture_on_start {
                state.finished =
                    BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
            }
            let _ = state
                .wake
                .publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary);
        }
        Ok(())
    }

    fn perform<I: Copy, const CAPACITY: usize>(
        &mut self,
        _items: &SchedulerItemSpace<'_>,
        _step: &SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError> {
        Ok(())
    }

    fn observe_wait(
        &mut self,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        Err(SchedulerHardwareError::NotAwaiting(wait))
    }

    fn recover(&mut self, _fault: SchedulerTransactionFault) {}

    fn step_stop(
        &mut self,
        stop: BluetoothSchedulerStop,
    ) -> Result<BluetoothSchedulerStopStep, oer_esp32s31_bluetooth::scheduler::SchedulerStopError>
    {
        let mut state = self.0.borrow_mut();
        if state.stop_never_settles {
            return Ok(BluetoothSchedulerStopStep::Pending(stop));
        }
        state.stops += 1;
        state.running = false;
        Ok(BluetoothSchedulerStopStep::Stopped(
            BluetoothSchedulerStopped::for_validation(),
        ))
    }
}

/// The scheduler policy and sleep clock of the test runtime.
enum TestConfig {}

impl oer_esp32s31_bluetooth_radio::BluetoothRadioConfig for TestConfig {
    const SCHEDULER: SchedulerSoftwareConfig = SchedulerSoftwareConfig::reviewed_standalone();
    const LOCAL_SLEEP_CLOCK_PPM: u16 = 500;
}

/// The capabilities of the test runtime's port.
type Caps = oer_esp32s31_bluetooth_radio::BluetoothCapabilities<TestConfig, 1, 1, 1, 1, 2, 3, 8>;

type Runtime = BluetoothRuntime<
    NoopRawMutex,
    Model,
    &'static VirtualClock,
    TestConfig,
    1,
    1,
    1,
    1,
    2,
    3,
    8,
    4,
>;

/// `value` checked against [`Caps`] and placed at the epoch's start, as the
/// Controller core places its requests.
fn placed<T: oer_bluetooth_radio::Timed>(value: T) -> Supported<Caps, T> {
    Supported::placed(value, at(0)).expect("the test request is placed")
}

/// `value` checked against [`Caps`].
fn supported<T: oer_bluetooth_radio::Untimed>(value: T) -> Supported<Caps, T> {
    Supported::new(value).expect("the test value is supported")
}

std::thread_local! {
    /// The virtual time of the test running on this thread.
    static CLOCK: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
}

fn clock() -> &'static VirtualClock {
    CLOCK.with(|clock| *clock)
}

/// Run the runtime for one millisecond of virtual time: poll it, moving
/// time to each deadline it waits for within the millisecond.
fn run_for_a_millisecond(runtime: &Fixture) {
    let end = clock()
        .now()
        .checked_add(oer_time::Duration::from_millis(1))
        .unwrap();
    let mut run = core::pin::pin!(runtime.run());
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..1_000 {
        assert!(
            run.as_mut().poll(&mut context).is_pending(),
            "the runtime keeps running"
        );
        match clock().next_deadline() {
            Some(deadline) if deadline <= end => clock().advance_to(deadline),
            _ => {}
        }
    }
    clock().advance_to(end);
}

const NONCONN: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];

fn configure() -> RadioRequest<'static, Caps> {
    RadioRequest::ConfigureAdvertising(supported(AdvertisingConfiguration {
        set: AdvertisingSetId::new(0),
        pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
        reception: AdvertisingReception::None,
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    }))
}

/// The radio's first instant: its epoch starts 2^32 us above zero.
const EPOCH_START: u64 = 1 << 32;

/// The instant `micros` after the radio epoch's start.
fn at(micros: u64) -> LeInstant {
    LeInstant::from_micros(EPOCH_START + micros)
}

fn scanner_configuration() -> RadioRequest<'static, Caps> {
    RadioRequest::ConfigureScanner(supported(ScannerConfiguration {
        scanner: ScannerId::new(0),
        scan_type: ScanType::Passive,
        filter_policy: ScanFilterPolicy::AcceptAll,
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    }))
}

fn scan_window(id: u32, start: u64) -> RadioRequest<'static, Caps> {
    RadioRequest::Scan(placed(oer_bluetooth_radio::ScanWindow {
        id: EventId::new(id),
        scanner: ScannerId::new(0),
        channel: AdvertisingChannel::Channel37,
        window: LeWindow::new(at(start), RadioDuration::from_micros(5_000)).unwrap(),
    }))
}

fn advertise(id: u32, anchor: u64) -> RadioRequest<'static, Caps> {
    RadioRequest::Advertise(placed(AdvertisingEvent {
        id: EventId::new(id),
        set: AdvertisingSetId::new(0),
        anchor: at(anchor),
        channels: AdvertisingChannels::single(AdvertisingChannel::Channel37),
        channel_spacing: RadioDuration::from_micros(1_000),
        coexistence: CoexistenceLevel::Baseline,
    }))
}

type Driver = crate::BluetoothDriver<
    'static,
    NoopRawMutex,
    Model,
    &'static VirtualClock,
    TestConfig,
    1,
    1,
    1,
    1,
    2,
    3,
    8,
    4,
>;

type Memory = oer_esp32s31_bluetooth_radio::BluetoothRadioMemory<1, 1, 1, 1, 2, 3>;

/// A runtime and the driver of its installed radio, which every call
/// polls beside the runtime's operation, as the port's task does.
struct Fixture {
    runtime: &'static Runtime,
    driver: core::cell::Cell<Option<Driver>>,
}

/// The fixture's driver, lent out of its cell while a call polls it; it
/// goes back on drop, also when the call's future is dropped unfinished.
struct Lent<'f> {
    cell: &'f core::cell::Cell<Option<Driver>>,
    driver: Option<Driver>,
}

impl Lent<'_> {
    fn get(&mut self) -> &mut Driver {
        self.driver.as_mut().expect("a lent driver")
    }
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        self.cell.set(self.driver.take());
    }
}

impl Fixture {
    fn lend(&self) -> Lent<'_> {
        Lent {
            cell: &self.driver,
            driver: Some(self.driver.take().expect("a radio is installed")),
        }
    }

    /// Poll `call` and the driver until `call` completes; the driver may
    /// stop first only on a fault, which a poisoned call then reports.
    async fn served<F: Future>(&self, call: F) -> F::Output {
        let mut driver = self.lend();
        let mut call = core::pin::pin!(call);
        match select(call.as_mut(), driver.get().run(|| false)).await {
            Either::First(output) => output,
            Either::Second(exit) => {
                assert!(exit.is_err(), "the driver returned only on a fault");
                call.await
            }
        }
    }

    async fn install(
        &self,
        memory: Memory,
        hardware: Model,
    ) -> Result<(), (crate::BluetoothInstallError, Memory, Model)> {
        let mut backend = self.runtime.install(memory, hardware).await?;
        self.driver.set(Some(backend.take_driver()));
        Ok(())
    }

    async fn submit_request(
        &self,
        request: RadioRequest<'_, Caps>,
    ) -> oer_bluetooth_radio::PortResult<(), RequestError, BluetoothFault<()>> {
        self.served(self.runtime.submit_request(request)).await
    }

    async fn read_now(
        &self,
    ) -> oer_bluetooth_radio::PortResult<LeInstant, ClockError, BluetoothFault<()>> {
        self.served(self.runtime.read_now()).await
    }

    async fn cancel_event(
        &self,
        id: EventId,
    ) -> oer_bluetooth_radio::PortResult<(), oer_bluetooth_radio::CancelError, BluetoothFault<()>>
    {
        self.served(self.runtime.cancel_event(id)).await
    }

    async fn enable(
        &self,
    ) -> oer_bluetooth_port::EnableResult<core::convert::Infallible, BluetoothFault<()>> {
        self.served(self.runtime.enable()).await
    }

    async fn disable(
        &self,
        sink: &mut dyn FnMut(BluetoothOutcome),
    ) -> Result<(), Poisoned<BluetoothFault<()>>> {
        self.served(self.runtime.disable(sink)).await
    }

    /// Drive the radio until the driver stops on a fault.
    async fn run(&self) -> crate::BluetoothRuntimeFault<()> {
        let mut driver = self.lend();
        match driver.get().run(|| false).await {
            Err(fault) => fault,
            Ok(()) => panic!("the driver ran until a fault"),
        }
    }

    /// Retire the driver and take the radio and hardware out.
    #[allow(clippy::type_complexity, reason = "the test radio's pools")]
    async fn take(
        &self,
    ) -> Result<
        (
            oer_esp32s31_bluetooth_radio::BluetoothRadio<1, 1, 1, 1, 2, 3, 8>,
            Model,
        ),
        crate::BluetoothUninstallError<()>,
    > {
        let mut driver = self.driver.take().expect("a radio is installed");
        let _stopped = driver.run(|| true).await;
        driver.into_parts().map_err(|retained| retained.error)
    }

    async fn with_radio<R>(
        &self,
        entry: impl FnOnce(&mut oer_esp32s31_bluetooth_radio::BluetoothRadio<1, 1, 1, 1, 2, 3, 8>) -> R,
    ) -> R {
        self.lend().get().with_radio(entry)
    }

    fn wait_event(
        &self,
    ) -> impl Future<
        Output = oer_bluetooth_radio::PortResult<
            BluetoothOutcome,
            oer_bluetooth_radio::EventsLost,
            BluetoothFault<()>,
        >,
    > + '_ {
        self.runtime.wait_event()
    }
}

/// A runtime for the model's scheduler policy and sleep clock, with no
/// radio installed.
fn new_runtime() -> Fixture {
    Fixture {
        runtime: std::boxed::Box::leak(std::boxed::Box::new(Runtime::new(clock()))),
        driver: core::cell::Cell::new(None),
    }
}

/// A runtime with the model's radio installed but its port disabled.
fn installed_disabled(model: &Model) -> Fixture {
    let runtime = new_runtime();
    block_on(runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the first install succeeds"));
    runtime
}

/// A runtime with the model's radio installed and its port enabled.
fn installed(model: &Model) -> Fixture {
    let runtime = installed_disabled(model);
    enable(&runtime);
    runtime
}

fn enable(runtime: &Fixture) {
    assert_eq!(block_on(runtime.enable()), Ok(Ok(())));
    // A transition queues no outcome.
    assert_eq!(taken(runtime), None);
}

/// The next outcome already queued, if any.
fn taken(runtime: &Fixture) -> Option<BluetoothOutcome> {
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    match core::pin::pin!(runtime.wait_event()).poll(&mut context) {
        core::task::Poll::Ready(Ok(Ok(outcome))) => Some(outcome),
        core::task::Poll::Ready(other) => panic!("an outcome expected: {other:?}"),
        core::task::Poll::Pending => None,
    }
}

#[test]
fn install_publishes_the_chains_once() {
    let model = Model::default();
    let runtime = installed(&model);
    assert!(model.0.borrow().chains_published);
    let Err((error, _, _)) = block_on(runtime.install(model_memory(), model.clone())) else {
        panic!("a second install is refused")
    };
    assert_eq!(error, BluetoothInstallError::AlreadyInstalled);

    let refusing = Model::default();
    refusing.0.borrow_mut().refuse_chains = true;
    let Err((error, _, _)) = block_on(new_runtime().install(model_memory(), refusing)) else {
        panic!("a refused chain publication fails the install")
    };
    assert_eq!(
        error,
        BluetoothInstallError::RxChains(RxChainPublicationError::SchedulerStarted)
    );
}

#[test]
fn an_admitted_event_runs_and_ends() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
        let outcome = match select(runtime.run(), runtime.wait_event()).await {
            Either::First(fault) => panic!("the runtime faulted: {fault:?}"),
            Either::Second(outcome) => outcome.unwrap().unwrap(),
        };
        assert_eq!(
            outcome,
            BluetoothOutcome::EventEnded {
                id: EventId::new(1),
                result: EventResult::Executed { anchor: None },
            }
        );
    });
    assert_eq!(model.0.borrow().started.len(), 1);
}

#[test]
fn an_accepted_scanner_publishes_the_scan_start_once() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    assert_eq!(model.0.borrow().scan_starts, 0);
    let scanner = || {
        RadioRequest::ConfigureScanner(supported(ScannerConfiguration {
            scanner: ScannerId::new(0),
            scan_type: ScanType::Active,
            filter_policy: ScanFilterPolicy::AcceptAll,
            tx_power: TxPower::from_dbm(0),
            phy: LePhy::Le1M,
        }))
    };
    block_on(runtime.submit_request(scanner()))
        .unwrap()
        .unwrap();
    assert_eq!(model.0.borrow().scan_starts, 1);
    // A refused configuration publishes nothing.
    assert!(matches!(
        block_on(runtime.submit_request(scanner())),
        Ok(Err(_))
    ));
    assert_eq!(model.0.borrow().scan_starts, 1);
}

#[test]
fn installation_and_every_accepted_list_change_publish_the_device_table() {
    let model = Model::default();
    let runtime = installed(&model);
    let counts = || {
        model
            .0
            .borrow()
            .device_tables
            .iter()
            .map(|publication| publication.count.get())
            .collect::<Vec<_>>()
    };
    // The empty table is published before any scanner can filter by it.
    assert_eq!(counts(), [0]);
    let device = |index: u8| AcceptListDevice {
        random: true,
        address: [index, 0, 0, 0, 0, 0xc0],
    };
    for index in 0..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u8 {
        block_on(
            runtime.submit_request(RadioRequest::FilterAcceptList(supported(
                AcceptListChange::Add(device(index)),
            ))),
        )
        .unwrap()
        .unwrap();
    }
    assert_eq!(
        counts().last().copied(),
        Some(BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u32)
    );
    let published = counts().len();
    // A refused addition publishes nothing.
    assert!(matches!(
        block_on(
            runtime.submit_request(RadioRequest::FilterAcceptList(supported(
                AcceptListChange::Add(device(0xff))
            )))
        ),
        Ok(Err(RequestError::ListFull))
    ));
    assert_eq!(counts().len(), published);
    block_on(
        runtime.submit_request(RadioRequest::FilterAcceptList(supported(
            AcceptListChange::Remove(device(0)),
        ))),
    )
    .unwrap()
    .unwrap();
    block_on(
        runtime.submit_request(RadioRequest::FilterAcceptList(supported(
            AcceptListChange::Clear,
        ))),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        counts()[published..],
        [BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u32 - 1, 0]
    );
    let tables = model.0.borrow();
    assert!(
        tables
            .device_tables
            .iter()
            .all(|publication| publication.first_entry == tables.device_tables[0].first_entry)
    );
}

#[test]
fn an_exhausted_epoch_admits_nothing_and_still_settles_admitted_work() {
    let model = Model::default();
    let runtime = installed(&model);
    assert_eq!(block_on(runtime.submit_request(configure())), Ok(Ok(())));
    assert_eq!(
        block_on(runtime.submit_request(advertise(1, 10_000))),
        Ok(Ok(()))
    );
    block_on(runtime.with_radio(|radio| radio.reach_epoch_end_for_validation()));
    model.0.borrow_mut().time = 2_000;
    assert_eq!(
        block_on(runtime.read_now()),
        Ok(Err(ClockError::EpochExhausted))
    );
    // It stays exhausted, whatever the next sample.
    model.0.borrow_mut().time = 0;
    assert_eq!(
        block_on(runtime.read_now()),
        Ok(Err(ClockError::EpochExhausted))
    );
    assert_eq!(
        block_on(runtime.submit_request(advertise(2, 20_000))),
        Ok(Err(RequestError::EpochExhausted))
    );
    // Disable still settles the admitted event.
    assert!(matches!(
        disable(&runtime).as_slice(),
        [BluetoothOutcome::EventEnded { id, .. }] if *id == EventId::new(1)
    ));
}

#[test]
fn the_clock_reports_a_fresh_radio_time() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().time = 2_000;
    let now = block_on(runtime.read_now()).unwrap().unwrap();
    // Two raw ticks per microsecond.
    assert_eq!(now, at(1_000));
    // The timing is the runtime's own, read before any install.
    assert_eq!(
        <Caps as oer_bluetooth_radio::LeCapabilities>::CAPS
            .timing
            .admission_guard,
        RadioDuration::from_micros(40)
    );
}

#[test]
fn a_refused_request_reports_the_radio_error() {
    let model = Model::default();
    model.0.borrow_mut().time = 20_000;
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        // The fresh sample places the anchor in the past.
        assert_eq!(
            runtime.submit_request(advertise(1, 5_000)).await,
            Ok(Err(RequestError::Late))
        );
    });
}

#[test]
fn disable_hands_every_admitted_events_end_to_its_sink() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    // The waiting event ends at once, through the sink: the phase's
    // consumer finds nothing left.
    assert_eq!(
        disable(&runtime),
        [BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::NotExecuted,
        }]
    );
    assert_eq!(taken(&runtime), None);
    // Enable resumes the scheduler the disable stopped.
    assert_eq!(model.0.borrow().stops, 1);
    enable(&runtime);
    block_on(runtime.submit_request(advertise(2, 20_000)))
        .unwrap()
        .unwrap();
}

#[test]
fn disable_hands_the_outcomes_the_phase_left_before_the_withdrawn_ends() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    // The first event runs and its end waits in the queue, untaken.
    block_on(runtime.submit_request(advertise(1, 10_000)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    model.0.borrow_mut().defer_execution = true;
    block_on(runtime.submit_request(advertise(2, 40_000)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(
        disable(&runtime),
        [
            BluetoothOutcome::EventEnded {
                id: EventId::new(1),
                result: EventResult::Executed { anchor: None },
            },
            BluetoothOutcome::EventEnded {
                id: EventId::new(2),
                result: EventResult::NotExecuted,
            },
        ]
    );
}

#[test]
fn cancellation_withdraws_an_admitted_event_and_its_end_follows() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        block_on(runtime.cancel_event(EventId::new(2))),
        Ok(Err(oer_bluetooth_radio::CancelError::NotRunning))
    );
    assert_eq!(block_on(runtime.cancel_event(EventId::new(1))), Ok(Ok(())));
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::NotExecuted,
        })
    );
}

#[test]
fn a_runner_fault_poisons_the_port_with_its_cause() {
    let model = Model::default();
    model.0.borrow_mut().refuse_start = true;
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    let fault = block_on(runtime.run());
    let poisoned = Poisoned {
        cause: BluetoothFault::Runner(fault),
    };
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        core::pin::pin!(runtime.wait_event()).poll(&mut context),
        core::task::Poll::Ready(Err(poisoned))
    );
    assert_eq!(block_on(runtime.submit_request(configure())), Err(poisoned));
    assert_eq!(block_on(runtime.read_now()), Err(poisoned));
}

#[test]
fn an_oversized_pdu_becomes_a_memory_fault() {
    let pdu = vec![0; crate::MAX_PDU_BYTES + 1];
    let outcome = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &pdu,
            rssi_dbm: -40,
            captured_at: None,
        },
    });
    // The runtime's sink takes it as a memory-inconsistency fault.
    assert_eq!(outcome, None);
    let fits = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at: None,
        },
    })
    .unwrap();
    assert_eq!(fits.portable().clone(), {
        let BluetoothOutcome::Received { pdu, .. } = &fits else {
            panic!("the PDU fits")
        };
        RadioOutcome::Received {
            id: EventId::new(1),
            pdu: pdu.portable(),
        }
    });
}

#[test]
fn as_a_port_backend_its_client_answers_a_refusal() {
    let model = Model::default();
    model.0.borrow_mut().time = 20_000;
    let runtime = installed(&model);
    block_on(async {
        assert_eq!(runtime.submit_request(configure()).await, Ok(Ok(())));
        assert_eq!(
            runtime.submit_request(advertise(1, 5_000)).await,
            Ok(Err(RequestError::Late))
        );
    });
}

#[test]
fn as_a_port_backend_it_states_le_1m_and_hardware_acknowledgement() {
    use oer_bluetooth_radio::LinkAcknowledgement;

    let model = Model::default();
    let runtime = installed(&model);
    let capabilities = <Caps as oer_bluetooth_radio::LeCapabilities>::CAPS;
    assert!(
        capabilities.legacy_advertising.nonconnectable
            && capabilities.legacy_advertising.responsive
    );
    assert!(capabilities.active_scanning);
    assert!(capabilities.phys.contains(LePhy::Le1M));
    assert!(!capabilities.phys.contains(LePhy::Le2M));
    let connection = capabilities
        .peripheral_connection
        .expect("the runtime has a connection");
    assert_eq!(
        connection.link_acknowledgement,
        LinkAcknowledgement::Hardware
    );
    // Every PDU the radio can carry fits an owned outcome.
    assert_eq!(
        crate::MAX_PDU_BYTES,
        2 + usize::from(connection.max_data_payload)
    );

    let RadioRequest::ConfigureAdvertising(configuration) = configure() else {
        unreachable!()
    };
    // LE 2M advertising lies outside the capabilities: it cannot be built.
    assert_eq!(
        Supported::<Caps, _>::new(AdvertisingConfiguration {
            phy: LePhy::Le2M,
            ..*configuration.get()
        }),
        Err(oer_bluetooth_radio::Unplaced::Unsupported)
    );
    assert_eq!(block_on(runtime.submit_request(configure())), Ok(Ok(())));
}

#[test]
fn an_enabled_port_is_retained_with_its_radio_and_hardware() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    // An owner poisoned while enabled leaves admitted events owing their
    // ends: the retirement keeps the radio for the composition's fail-stop.
    assert_eq!(
        block_on(runtime.take()).err(),
        Some(crate::BluetoothUninstallError::NotDisabled)
    );
    assert_eq!(model.0.borrow().stops, 0);
}

#[test]
fn uninstall_stops_the_scheduler_and_a_reset_radio_installs_again() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    // The event is listed and running when the epoch ends.
    model.0.borrow_mut().defer_execution = true;
    run_for_a_millisecond(&runtime);
    assert_eq!(model.0.borrow().stops, 0);
    // The stop finds it started without a status: execution unconfirmed.
    model.0.borrow_mut().time = 2 * 20_000;
    assert_eq!(
        disable(&runtime),
        [BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::Aborted,
        }]
    );
    assert_eq!(model.0.borrow().stops, 1);
    let (radio, hardware) = block_on(runtime.take()).unwrap();
    // The stopped scheduler is not stopped again.
    assert_eq!(model.0.borrow().stops, 1);
    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());
    model.0.borrow_mut().chains_published = false;
    block_on(runtime.install(memory, hardware))
        .unwrap_or_else(|_| panic!("the next epoch installs"));
    assert!(model.0.borrow().chains_published);
    enable(&runtime);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

fn test_receive(id: u32) -> RadioRequest<'static, Caps> {
    RadioRequest::TestReceive(placed(TestReceive {
        id: EventId::new(id),
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
        window: LeWindow::new(at(10_000), RadioDuration::from_micros(1_000)).unwrap(),
        recurring: false,
        tx_power: TxPower::from_dbm(0),
    }))
}

fn routes(model: &Model) -> (usize, usize) {
    let state = model.0.borrow();
    (state.routes_disabled, state.routes_restored)
}

/// Run the runtime until it is idle for a moment.
fn settle(runtime: &Fixture) {
    run_for_a_millisecond(runtime);
}

/// Disable the port, as its owner's `disable` does: the outcomes it handed
/// to the sink.
fn disable(runtime: &Fixture) -> Vec<BluetoothOutcome> {
    let mut outcomes = Vec::new();
    assert_eq!(
        block_on(runtime.disable(&mut |outcome| outcomes.push(outcome))),
        Ok(())
    );
    outcomes
}

#[test]
fn a_test_session_holds_the_route_until_test_end() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 0));
    block_on(runtime.submit_request(RadioRequest::EndTest))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn cancelling_a_running_test_stops_the_scheduler_and_resumes_it() {
    let model = Model::default();
    model.0.borrow_mut().defer_execution = true;
    let runtime = installed(&model);
    let RadioRequest::TestReceive(test) = test_receive(4) else {
        unreachable!()
    };
    block_on(
        runtime.submit_request(RadioRequest::TestReceive(placed(TestReceive {
            recurring: true,
            ..*test.get()
        }))),
    )
    .unwrap()
    .unwrap();
    settle(&runtime);
    // The running test leaves its list only after a stop: the runner stops
    // the scheduler, resumes it and ends the event, with the port enabled.
    block_on(runtime.cancel_event(EventId::new(4)))
        .unwrap()
        .unwrap();
    assert_eq!(model.0.borrow().stops, 0);
    settle(&runtime);
    assert_eq!(model.0.borrow().stops, 1);
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(4),
            result: EventResult::NotExecuted,
        })
    );
    // The resumed scheduler admits the session's end and new work.
    block_on(runtime.submit_request(RadioRequest::EndTest))
        .unwrap()
        .unwrap();
    settle(&runtime);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    assert_eq!(model.0.borrow().stops, 1);
}

#[test]
fn uninstall_restores_the_route_of_an_open_test() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 0));
    disable(&runtime);
    let _ = block_on(runtime.take()).unwrap();
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn a_fault_restores_the_route_of_an_open_test() {
    let model = Model::default();
    model.0.borrow_mut().refuse_start = true;
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    assert!(matches!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::Start(_)
    ));
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn owned_outcomes_keep_the_pdu_its_stamp_and_an_aborted_end() {
    let captured_at = Some(at(0));
    let received = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(3),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at,
        },
    })
    .unwrap();
    let BluetoothOutcome::Received { pdu, .. } = &received else {
        panic!("PDU is retained")
    };
    assert_eq!(pdu.pdu(), &NONCONN);
    assert_eq!(pdu.portable().captured_at, captured_at);
    let ended = BluetoothOutcome::copy(RadioOutcome::EventEnded {
        id: EventId::new(3),
        result: EventResult::Aborted,
    })
    .unwrap();
    assert_eq!(
        ended.portable(),
        RadioOutcome::EventEnded {
            id: EventId::new(3),
            result: EventResult::Aborted
        }
    );
}

#[test]
fn stop_drains_the_prior_finished_list_before_capturing_the_final_snapshot() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().finished =
        BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
    disable(&runtime);
    assert!(model.0.borrow().finished.is_none());
    assert_eq!(model.0.borrow().stops, 1);
    enable(&runtime);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

#[test]
fn pass_drains_prior_finished_work_before_consuming_the_next_wake() {
    let model = Model::default();
    model.0.borrow_mut().capture_on_start = true;
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    block_on(runtime.submit_request(advertise(19, 10_000)))
        .unwrap()
        .unwrap();
    // A prior captured list and the next interrupt coexist. Draining the
    // prior owner must precede consuming that interrupt's transfer.
    run_for_a_millisecond(&runtime);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        core::pin::pin!(runtime.wait_event()).poll(&mut context),
        core::task::Poll::Ready(Ok(Ok(BluetoothOutcome::EventEnded {
            id: EventId::new(19),
            result: EventResult::Executed { anchor: None },
        })))
    );
    assert!(
        core::pin::pin!(runtime.wait_event())
            .poll(&mut context)
            .is_pending()
    );
    assert!(model.0.borrow().finished.is_none());
    assert!(model.0.borrow().wake.take().is_none());
}

#[test]
fn a_wait_past_the_timer_range_is_an_error_not_a_forever_wait() {
    let clock: VirtualClock =
        VirtualClock::starting_at(oer_time::Instant::from_micros(u64::MAX - 5));
    assert_eq!(
        block_on(crate::runtime::wait_for(&clock, crate::HARDWARE_RECHECK)),
        Err(crate::BluetoothTimeError::Deadline)
    );
    let clock: VirtualClock = VirtualClock::new();
    let wait = crate::runtime::wait_for(&clock, crate::HARDWARE_RECHECK);
    let mut wait = core::pin::pin!(wait);
    assert!(embassy_futures::poll_once(wait.as_mut()).is_pending());
    clock.advance(crate::HARDWARE_RECHECK).unwrap();
    assert_eq!(block_on(wait), Ok(()));
}

/// The body of a port that admits one configuration and ends.
struct ConfigureOnce;

type Backend = crate::BluetoothBackend<'static, Runtime, Driver>;

impl oer_bluetooth_port::PortBody<Backend> for ConfigureOnce {
    async fn run<'p, 'id>(
        self,
        owner: oer_bluetooth_port::Owner<'p, 'id, Backend, oer_bluetooth_port::Disabled>,
        (): (),
    ) -> oer_bluetooth_port::Ended<'id, Backend> {
        use oer_bluetooth_radio::LeRadioBackend;

        let Ok(mut owner) = owner.enable().await else {
            panic!("the installed port enables")
        };
        {
            let mut parts = owner.split();
            assert_eq!(
                Backend::submit(&mut *parts.clients, configure()).await,
                Ok(Ok(()))
            );
        }
        let Ok(owner) = owner.disable(&mut |_| {}).await else {
            panic!("the port disables")
        };
        owner.uninstall(())
    }
}

/// Install returns the port's backend with its driver inside; `run_port`
/// joins the driver with the body, retires it after the body's uninstall
/// and returns the radio and hardware for the next install.
#[test]
fn a_port_body_runs_beside_its_driver_and_the_uninstall_returns_the_radio() {
    let model = Model::default();
    let runtime = new_runtime();
    let backend = block_on(runtime.runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the first install succeeds"));
    let Some(Ok((radio, hardware))) =
        block_on(oer_bluetooth_port::run_port(backend, ConfigureOnce))
    else {
        panic!("the body uninstalls the port, and the driver leaves its radio")
    };
    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());
    let backend = block_on(runtime.runtime.install(memory, hardware))
        .unwrap_or_else(|_| panic!("the next epoch installs"));
    // The next port starts with an empty queue ().
    assert_eq!(taken(&runtime), None);
    assert!(matches!(
        block_on(oer_bluetooth_port::run_port(backend, ConfigureOnce)),
        Some(Ok(_))
    ));
}

#[test]
fn an_abandoned_owner_poisons_the_runtime_and_refuses_every_install() {
    let model = Model::default();
    let runtime = installed(&model);
    runtime.runtime.abandon();
    let poisoned = Poisoned {
        cause: BluetoothFault::Runner(crate::BluetoothRuntimeFault::Abandoned),
    };
    assert_eq!(block_on(runtime.read_now()), Err(poisoned));
    // The runner halts at its next pass.
    assert_eq!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::Abandoned
    );
    let Err((error, _, _)) = block_on(runtime.install(model_memory(), model.clone())) else {
        panic!("an abandoned runtime installs nothing")
    };
    assert_eq!(error, crate::BluetoothInstallError::Abandoned);
}

#[test]
fn a_disable_past_its_bound_poisons_the_runtime_and_halts_the_runner() {
    let model = Model::default();
    let runtime = installed(&model);
    // The stop sequence never ends.
    model.0.borrow_mut().stop_never_settles = true;
    let mut sink = |_| {};
    let mut disable = core::pin::pin!(runtime.disable(&mut sink));
    let started = clock().now();
    let ended = loop {
        if let core::task::Poll::Ready(ended) = embassy_futures::poll_once(disable.as_mut()) {
            break ended;
        }
        let deadline = clock().next_deadline().expect("the disable waits on time");
        clock().advance_to(deadline);
    };
    let poisoned = Poisoned {
        cause: BluetoothFault::Runner(crate::BluetoothRuntimeFault::TransitionTimedOut),
    };
    assert_eq!(ended, Err(poisoned));
    assert_eq!(
        clock().now(),
        started.checked_add(crate::TRANSITION_BOUND).unwrap()
    );
    assert_eq!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::TransitionTimedOut
    );
}

#[test]
fn disable_reports_real_completions_and_aborts_only_started_events() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    // The first event runs and records its status before the stop.
    block_on(runtime.submit_request(advertise(1, 10_000)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    let ended = taken(&runtime);
    assert_eq!(
        ended,
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::Executed { anchor: None },
        })
    );
    // The second one is listed and has not started when the port stops.
    model.0.borrow_mut().defer_execution = true;
    block_on(runtime.submit_request(advertise(2, 40_000)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(
        disable(&runtime),
        [BluetoothOutcome::EventEnded {
            id: EventId::new(2),
            result: EventResult::NotExecuted,
        }]
    );
    // Enable resumes the stopped scheduler and admits again.
    enable(&runtime);
    block_on(runtime.submit_request(advertise(3, 60_000)))
        .unwrap()
        .unwrap();
}

#[test]
fn an_event_without_room_for_its_ends_is_refused_and_disable_ends_the_rest() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    model.0.borrow_mut().defer_execution = true;
    block_on(runtime.submit_request(advertise(1, 10_000)))
        .unwrap()
        .unwrap();
    block_on(runtime.submit_request(scanner_configuration()))
        .unwrap()
        .unwrap();
    // Each event reserves two of the four slots.
    block_on(runtime.submit_request(scan_window(2, 30_000)))
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(runtime.submit_request(advertise(3, 50_000))),
        Ok(Err(RequestError::Busy))
    );
    // The consumer takes nothing before the stop, which ends each admitted
    // event exactly once.
    let mut ended: Vec<EventId> = disable(&runtime)
        .into_iter()
        .map(|outcome| match outcome {
            BluetoothOutcome::EventEnded { id, .. } => id,
            other => panic!("only ends expected: {other:?}"),
        })
        .collect();
    ended.sort_by_key(|id| id.get());
    assert_eq!(ended, [EventId::new(1), EventId::new(2)]);
    assert_eq!(taken(&runtime), None);
}

#[test]
fn each_transition_is_traced_from_its_start_to_how_it_ended() {
    use oer_bluetooth_trace::{Lifecycle, LifecycleCommand as Command, LifecycleStep as Step};

    let model = Model::default();
    let runtime = installed(&model);
    let step = |command, step| Lifecycle { command, step };
    assert_eq!(
        crate::trace::take::<Lifecycle>(),
        [
            step(Command::Enable, Step::Started),
            step(Command::Enable, Step::Done),
        ]
    );
    disable(&runtime);
    assert_eq!(
        crate::trace::take::<Lifecycle>(),
        [
            step(Command::Disable, Step::Started),
            step(Command::Disable, Step::Done),
        ]
    );
}

/// The driver serves the radio whatever the port's body awaits: with no
/// consumer taking an outcome, an admitted event still runs and its end is
/// queued.
#[test]
fn the_driver_runs_admitted_events_while_no_consumer_takes_their_ends() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    block_on(runtime.submit_request(advertise(1, 10_000)))
        .unwrap()
        .unwrap();
    assert!(model.0.borrow().started.is_empty());
    run_for_a_millisecond(&runtime);
    assert_eq!(model.0.borrow().started.len(), 1);
    assert!(matches!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded { id, .. }) if id == EventId::new(1)
    ));
}

/// A disabled port's driver keeps sampling the controller clock at least
/// every `TIME_REFRESH`, so the extension of its 32-bit counter holds.
#[test]
fn a_disabled_ports_driver_refreshes_the_controller_clock() {
    let model = Model::default();
    let runtime = installed_disabled(&model);
    let before = model.0.borrow().time_requests;
    let mut run = core::pin::pin!(runtime.run());
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(run.as_mut().poll(&mut context).is_pending());
    let first = model.0.borrow().time_requests;
    assert!(first > before, "the driver samples when it starts");
    clock()
        .advance(crate::TIME_REFRESH)
        .expect("the refresh fits the clock");
    assert!(run.as_mut().poll(&mut context).is_pending());
    assert!(
        model.0.borrow().time_requests > first,
        "the driver samples again after TIME_REFRESH while disabled"
    );
}

/// Readings of the clock beside each other share one sample the driver
/// takes after both were asked; no reader waits for another.
#[test]
fn readings_beside_each_other_share_one_fresh_sample() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().time = 4_000;
    let before = model.0.borrow().time_requests;
    let (first, second) = block_on(runtime.served(embassy_futures::join::join(
        runtime.runtime.read_now(),
        runtime.runtime.read_now(),
    )));
    assert_eq!(first, Ok(Ok(at(2_000))));
    assert_eq!(second, first);
    assert_eq!(model.0.borrow().time_requests, before + 1);
}

/// The body of a port that, once disabled, awaits something of its own
/// before it uninstalls: the driver has parked by then.
struct WaitThenUninstall;

impl oer_bluetooth_port::PortBody<Backend> for WaitThenUninstall {
    async fn run<'p, 'id>(
        self,
        owner: oer_bluetooth_port::Owner<'p, 'id, Backend, oer_bluetooth_port::Disabled>,
        (): (),
    ) -> oer_bluetooth_port::Ended<'id, Backend> {
        let Ok(owner) = owner.enable().await else {
            panic!("the installed port enables")
        };
        let Ok(owner) = owner.disable(&mut |_| {}).await else {
            panic!("the port disables")
        };
        // Something that reaches no port call, as a drained transport.
        for _ in 0..8 {
            embassy_futures::yield_now().await;
        }
        owner.uninstall(())
    }
}

/// A retire request wakes an idle driver at once: the uninstall of a body
/// that waited after its disable needs no time to pass.
#[test]
fn a_retire_request_wakes_an_idle_driver() {
    let model = Model::default();
    let runtime = new_runtime();
    let backend = block_on(runtime.runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the install succeeds"));
    let mut port = core::pin::pin!(oer_bluetooth_port::run_port(backend, WaitThenUninstall));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let started = clock().now();
    let ended = (0..1_000)
        .find_map(|_| match port.as_mut().poll(&mut context) {
            core::task::Poll::Ready(ended) => Some(ended),
            core::task::Poll::Pending => None,
        })
        .expect("the port ends without time passing");
    assert!(
        matches!(ended, Some(Ok(_))),
        "the driver retires within its bound"
    );
    assert_eq!(clock().now(), started);
}
