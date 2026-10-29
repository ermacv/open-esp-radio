// The image's station observations need the trace class its package
// declares; `cargo fw` and `cargo hil` set the cfg.
#[cfg(not(oer_trace))]
compile_error!(
    "the wifi-station class reports the station service's observations: build it with its declared trace class (RUSTFLAGS=\"--cfg oer_trace\")"
);

mod control;
mod future;
mod ip;
pub(crate) mod irq;
mod radio;
pub(crate) mod udp;
use crate::services::StationRequests as Requests;
use crate::transport::CONSOLE;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi::NonceSource;
use oer_hil_protocol::RequestIdentity;
use oer_ieee80211_control::{
    StationLink, StationLinkState, StationRole, StationServiceEnd, StationServiceEnded,
    StationStart, StopStation,
};
use oer_time::Clock;

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::peripherals::software_interrupt::FromCpu3Irq;
use oer_esp32s31_hal::peripherals::watchdog::{ArmedRtcWatchdog, RwdtRegisters};
use oer_esp32s31_wifi::{self as wifi, InterruptTypes};
use oer_espressif_executor::{Doorbell, InterruptExecutor};

use oer_memory::{Placed, StaticPlace};
use static_cell::{ConstStaticCell, StaticCell};
static NONCES: ConstStaticCell<StaticPlace<NonceSource>> = ConstStaticCell::new(StaticPlace::new());

#[derive(Clone, Copy)]
pub struct RadioConfig {
    pub channel: u8,
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
// The laboratory image's regulatory installation; no credentials are built in.
const CONFIG: RadioConfig = RadioConfig {
    channel: 1,
    country: *b"DE",
    cap_dbm: 10,
};
const RECONNECT: wifi::ReconnectPolicy =
    match wifi::ReconnectPolicy::every(oer_time::Duration::from_secs(1)) {
        Ok(policy) => policy,
        Err(_) => panic!("the fixed reconnect interval is nonzero"),
    };

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    pub(crate) RadioTasks: InterruptExecutor<Esp32s31, 2> {
        @observer: crate::measurements::CpuObserver;
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(driver: wifi::Driver<radio::binding::Binding>) = radio::drive;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    pub(crate) AppTasks: Executor<Esp32s31, 0> {
        @observer: crate::measurements::CpuObserver;
        console(work: oer_memory::PlacedFuture<'static, ConsoleWork>) = console;
        station_task(work: oer_memory::PlacedFuture<'static, StationWork>) = station_task;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = crate::feed_rtc_watchdog;
        capture(observations: &'static ObservationStorage) = capture;
        reset_on_request() = crate::system::panic::reset_on_request;
    }
}

static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = wifi::RadioActor<crate::phy_pilot::Client>;

pub(crate) fn start(
    handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>,
) -> ! {
    let prepared = crate::initialize_runtime();
    let (
        (
            runtime_aes,
            runtime_efuse,
            runtime_radio,
            runtime_clocks,
            runtime_timg0_alarm,
            runtime_system_timer,
            runtime_software_interrupts,
            runtime_wifi_interrupts,
            runtime_entropy,
            runtime_watchdogs,
        ),
        _psram,
        mut adopted,
    ) = crate::finish_startup(|| {
        prepared.adopt(
            handoff,
            crate::INTERRUPT_TABLE,
            board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                crate::system::read_reset_causes(hardware.reset_cause);
                crate::system::take_platform_panic();
                crate::fatal::report_previous();
                oer_esp32s31_compose::wifi_fault::report_previous();
                (
                    hardware.aes,
                    hardware.efuse,
                    hardware.radio,
                    hardware.clocks,
                    hardware.timg0_alarm,
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.wifi_interrupts,
                    hardware.entropy,
                    hardware.watchdogs,
                )
            },
        )
    })
    .into_parts();
    crate::exception::report_stack_guard();
    let silent = adopted.router().silent::<oer_esp32s31_hal::peripherals::aes::AesInterrupt>()
        .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::InterruptTable));
    let engine = oer_esp32s31_compose::crypto::wifi_key_unwrap(runtime_aes, silent);
    let config = CONFIG;
    let Some(interrupts) = crate::Interrupts::take() else {
        crate::fail(crate::fatal::Failure::InterruptTable);
    };
    let factory = oer_esp32s31_compose::platform::DeviceIdentity::read(runtime_efuse);
    let station = factory.station_address();
    let ap = factory.ap_address();
    let radio_resources = wifi::RadioResources::new::<radio::binding::Binding>(
        runtime_radio,
        runtime_clocks,
        &factory,
    );
    let phy_pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let (actor, hardware) = radio_resources.start(phy_pilot);
    let actor = ACTOR.init(actor);
    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    oer_esp32s31_compose::platform::timers(
        runtime_system_timer,
        adopted.router(),
        interrupts.alarm,
        interrupts.radio_alarm,
    );
    let software = runtime_software_interrupts;
    // The radio handler's part holds both banks and the doorbell's lowering
    // half; its binding makes the doorbell's ringing half.
    let (line, lower) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            wifi::Interrupts::part(runtime_wifi_interrupts, software.irq3, wifi::Lower(lower)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let bell = wifi::Bell(bell);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let mut rng = oer_esp32s31_compose::platform::Entropy::new(runtime_entropy, EmbassyClock);
    if !rng.is_healthy() {
        crate::fail(crate::fatal::Failure::EntropySource);
    }
    let nonces = Placed::init(NONCES.take().uninit(), |place| {
        place.write(NonceSource::from_entropy(|seed| rng.fill(seed)))
    });
    let mut boot_bytes = [0; 8];
    rng.fill(&mut boot_bytes);
    let seed = u64::from_le_bytes(boot_bytes);
    let boot = seed.max(1);
    let mut tx_bytes = [0; 4];
    rng.fill(&mut tx_bytes);
    let entropy = u32::from_le_bytes(tx_bytes);
    CONSOLE.start(boot);
    let setup = wifi::Config::new(config.channel, config.country, config.cap_dbm, None, true)
        .unwrap_or_else(|error| radio::failure(error))
        .with_reconnect(RECONNECT);
    let observations = OBSERVATIONS
        .try_take()
        .unwrap_or_else(|| radio::failure(wifi::InitializationError::Storage));
    let observations: &'static ObservationStorage = observations;
    let pending: &'static oer_ieee80211_control::PendingCommands =
        match PENDING.try_init(oer_ieee80211_control::PendingCommands::new()) {
            Some(pending) => pending,
            None => radio::failure(wifi::InitializationError::Control),
        };
    let (control, net, runners) = wifi::station(
        wifi::Resources {
            engine,
            events: control::ProductEvents(pending),
            storage: radio::binding::storage(),
            hardware,
            rng,
            address: station,
            ap,
            bound,
            bell,
            entropy,
            interrupts: &irq::LINE,
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
            dma: wifi::DmaResources::claim()
                .unwrap_or_else(|error| radio::failure(wifi::InitializationError::Dma(error))),
        },
        setup,
    );
    if RadioTasks::radio_actor(&radio, actor).is_err()
        || RadioTasks::driver(&radio, runners.driver).is_err()
    {
        crate::fail(crate::fatal::Failure::TaskAllocation);
    }
    let status = control.status_reader();
    let (usb, _pads) = crate::usb::open(adopted.router(), interrupts.console_usb);
    let executor = crate::Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => crate::fail(crate::fatal::Failure::from_boot(error)),
    };
    let watchdog = runtime_watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            urgent,
            move |spawner| {
                let console_work = CONSOLE_WORK
                    .take()
                    .future(|| console_future(usb, boot, status, observations));
                if AppTasks::console(spawner, console_work).is_err() {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
                let station_work = STATION_WORK.take().future(|| {
                    station_work_future(StationInputs {
                        runner: runners.station,
                        control,
                        net,
                        address: station,
                        seed,
                        nonces,
                        pending,
                        observations,
                    })
                });
                if AppTasks::station_task(spawner, station_work).is_err() {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
                if AppTasks::capture(spawner, observations).is_err()
                    || AppTasks::reset_on_request(spawner).is_err()
                {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
                if AppTasks::rtc_watchdog(spawner, watchdog).is_err() {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}

#[derive(Clone, Copy)]
pub struct Attempt {
    pub request: RequestIdentity,
}
static EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
impl Attempt {
    fn advance(self) {
        use core::sync::atomic::Ordering;
        EPOCH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
                epoch.checked_add(1)
            })
            .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control));
    }
    pub fn publish(self, state: StationLinkState) {
        CONSOLE.publish(
            self.request.session_id,
            self.request.request_id,
            &StationLink {
                epoch: EPOCH.load(core::sync::atomic::Ordering::Relaxed),
                state,
            },
        );
    }
}

/// Set once the station service ended without a stop command; no task
/// answers a role command after it.
static ENDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
fn end(attempt: Attempt, end: StationServiceEnd) {
    use core::sync::atomic::Ordering;
    ENDED.store(true, Ordering::Release);
    emit(
        attempt.request,
        &StationServiceEnded {
            epoch: EPOCH.load(Ordering::Relaxed),
            end,
        },
    );
}
fn role(configured: bool, stopping: bool) -> StationRole {
    if !configured {
        StationRole::Idle
    } else if ENDED.load(core::sync::atomic::Ordering::Acquire) {
        StationRole::Ended
    } else if stopping {
        StationRole::Stopping
    } else {
        StationRole::Running
    }
}

type Starts = embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (RequestIdentity, StationStart),
    1,
>;
static STARTS: Starts = Starts::new();
type Controls<T> = embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    T,
    1,
>;
static COMMANDS: Controls<(RequestIdentity, oer_ieee80211_control::StationCommandKind)> =
    Controls::new();
static STOPS: Controls<RequestIdentity> = Controls::new();
static PENDING: StaticCell<oer_ieee80211_control::PendingCommands> = StaticCell::new();
fn post_command(
    request: RequestIdentity,
    command: oer_ieee80211_control::StationCommandKind,
    role: StationRole,
) {
    if role != StationRole::Running {
        emit(
            request,
            &oer_hil_protocol::base::Rejected(oer_hil_protocol::base::RejectReason::InvalidState),
        );
        return;
    }
    if COMMANDS.try_send((request, command)).is_err() {
        emit(
            request,
            &oer_hil_protocol::base::Rejected(oer_hil_protocol::base::RejectReason::Busy),
        );
    }
}

type ConsoleWork = impl core::future::Future<Output = ()>;
static CONSOLE_WORK: ConstStaticCell<StaticPlace<ConsoleWork>> =
    ConstStaticCell::new(StaticPlace::new());
#[define_opaque(ConsoleWork)]
fn console_future(
    usb: crate::transport::Usb,
    boot: u64,
    status: wifi::StatusReader<'static>,
    observations: &'static ObservationStorage,
) -> ConsoleWork {
    console_work(usb, boot, status, observations)
}
/// The console's task: its work lives in its own static place.
async fn console(work: oer_memory::PlacedFuture<'static, ConsoleWork>) {
    work.await;
}
async fn console_work(
    usb: crate::transport::Usb,
    boot: u64,
    status: wifi::StatusReader<'static>,
    observations: &'static ObservationStorage,
) {
    use oer_hil_protocol::base::{Accepted, RejectReason, Rejected};
    let mut configured = false;
    let mut stopping = false;
    crate::transport::serve(
        usb,
        boot,
        crate::services::STATION_KEYS,
        0,
        move |request,
              command: oer_hil_agent::base::Either<
            Requests,
            oer_hil_agent::base::Either<
                oer_hil_agent::capture::Request,
                oer_hil_agent::base::Either<oer_hil_agent::lifecycle::Request, udp::Requests>,
            >,
        >| {
            use oer_hil_agent::base::Either;
            let command = match command {
                Either::First(command) => command,
                Either::Second(Either::First(command)) => {
                    if CAPTURES.try_send((request, command)).is_err() {
                        emit(request, &Rejected(RejectReason::Busy));
                    }
                    return;
                }
                Either::Second(Either::Second(Either::First(_))) => {
                    crate::system::panic::queue(request);
                    return;
                }
                Either::Second(Either::Second(Either::Second(command))) => {
                    udp::request(
                        request,
                        command,
                        role(configured, stopping) == StationRole::Running,
                    );
                    return;
                }
            };
            let reason = match command {
                Requests::Counters(_) => {
                    emit(request, &counters(observations));
                    return;
                }
                Requests::Start(start) => {
                    if configured {
                        Some(RejectReason::InvalidState)
                    } else if start.0.credentials.validate().is_err() {
                        Some(RejectReason::InvalidConfiguration)
                    } else if STARTS.try_send((request, start.0)).is_err() {
                        Some(RejectReason::Busy)
                    } else {
                        configured = true;
                        None
                    }
                }
                Requests::Connect(_) => {
                    post_command(
                        request,
                        oer_ieee80211_control::StationCommandKind::Connect,
                        role(configured, stopping),
                    );
                    return;
                }
                Requests::Disconnect(_) => {
                    post_command(
                        request,
                        oer_ieee80211_control::StationCommandKind::Disconnect,
                        role(configured, stopping),
                    );
                    return;
                }
                Requests::Scan(_) => {
                    post_command(
                        request,
                        oer_ieee80211_control::StationCommandKind::BackgroundScan,
                        role(configured, stopping),
                    );
                    return;
                }
                Requests::Status(_) => {
                    emit(
                        request,
                        &oer_ieee80211_control::StationStatus::from(status.status()),
                    );
                    return;
                }
                Requests::Stop(_) => {
                    if let Some(reason) = StopStation::refusal(role(configured, stopping)) {
                        Some(reason)
                    } else if STOPS.try_send(request).is_err() {
                        Some(RejectReason::Busy)
                    } else {
                        stopping = true;
                        None
                    }
                }
            };
            if let Some(reason) = reason {
                emit(request, &Rejected(reason));
            } else {
                emit(request, &Accepted);
            }
        },
    )
    .await
}

/// The station's task: its work lives in its own static place.
async fn station_task(work: oer_memory::PlacedFuture<'static, StationWork>) {
    work.await;
}
type StationWork = impl core::future::Future<Output = ()>;
static STATION_WORK: ConstStaticCell<StaticPlace<StationWork>> =
    ConstStaticCell::new(StaticPlace::new());
struct StationInputs {
    runner: wifi::StationRunner<radio::binding::Binding>,
    control: wifi::Control<'static>,
    net: wifi::Network<'static>,
    address: [u8; 6],
    seed: u64,
    nonces: Placed<'static, NonceSource>,
    pending: &'static oer_ieee80211_control::PendingCommands,
    observations: &'static ObservationStorage,
}
#[define_opaque(StationWork)]
fn station_work_future(inputs: StationInputs) -> StationWork {
    station_work(inputs)
}
async fn station_work(inputs: StationInputs) {
    let StationInputs {
        runner,
        control,
        net,
        address,
        seed,
        nonces,
        pending,
        observations,
    } = inputs;
    let (request, mut start) = STARTS.receive().await;
    let attempt = Attempt { request };
    let settings = wifi::StationSettings::wpa2_passphrase(
        start.credentials.ssid(),
        start.credentials.passphrase(),
    )
    .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Ssid));
    start.credentials.clear_passphrase();
    drop(start);
    let begin = EmbassyClock.now();
    let credentials = settings
        .prepare()
        .await
        .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Ssid));
    let elapsed = EmbassyClock
        .now()
        .as_micros()
        .saturating_sub(begin.as_micros());
    CONSOLE
        .note_reliably(&oer_diagnostics::Note::station_pmk(elapsed))
        .await;
    let setup = wifi::Config::new(CONFIG.channel, CONFIG.country, CONFIG.cap_dbm, None, true)
        .unwrap_or_else(|error| radio::failure(error));
    let profile = wifi::Profile::new(
        &setup,
        address,
        credentials,
        control::Callbacks(nonces),
        StationObserver {
            attempt,
            observations,
        },
    )
    .unwrap_or_else(|error| radio::failure(error));
    control::serve(
        runner,
        control,
        net,
        profile,
        address,
        seed,
        control::Session {
            attempt,
            pending,
            observations,
        },
    )
    .await;
}

// Keep the bounded wire-record constructor out of control/worker poll frames.
#[inline(never)]
fn emit<M: oer_hil_protocol::Message>(request: RequestIdentity, body: &M) {
    CONSOLE.publish(request.session_id, request.request_id, body);
}

use oer_ieee80211_control::collector::Observations;
pub(super) type ObservationStorage = Observations<'static, 37>;
static COUNTERS: oer_hil_agent::capture::collector::Collector<37> = const {
    match oer_hil_agent::capture::collector::Collector::new([
        <oer_ieee80211_instrumentation::RxContainment as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RxReorder as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RxBaStarted as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RxBaStopped as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxRateSelected as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TokenLost as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::DataMsduLost as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RsnInputDropped as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::ScanBssObserved as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::PortEventsLost as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RxFrameDropped as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::RxMsduRefused as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxUnsent as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxMsduTooLong as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::GroupKeyInstalled as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::StationAuthenticated as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::StationBeacon as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::StationAssociated as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxIndeterminate as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxHardwareTimeout as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::TxQueueCollision as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::ControlDelivered as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::ArpAddresses as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::ArpTransmitted as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::ArpReceived as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::EspNowPeerTableChanged as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::EspNowPeersCleared as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::EspNowTxPacketNumber as oer_trace::Event>::KIND,
        <oer_ieee80211_instrumentation::EspNowReplay as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::Publication as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::TerminalPosted as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::EdcaTable as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::EdcaRecord as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::EdcaDraw as oer_trace::Event>::KIND,
        <oer_esp32s31_instrumentation::wifi::GroupRollback as oer_trace::Event>::KIND,
        <oer_ieee80211_control::collector::NetworkArp as oer_trace::Event>::KIND,
        <oer_ieee80211_control::collector::NetworkRefused as oer_trace::Event>::KIND,
    ]) { Ok(bank) => bank, Err(_) => panic!("unique image event kinds") }
};
static OBSERVATIONS: ConstStaticCell<ObservationStorage> = ConstStaticCell::new(Observations::new(
    &COUNTERS, oer_hil_protocol::capture::Identity { capture: 1, epoch: 0 },
));
#[derive(Default)]
pub struct DriverObserver;
impl oer_trace::Observer for DriverObserver {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        oer_trace::Observer::event(&COUNTERS, event);
    }
}
#[derive(Clone, Copy)]
pub struct StationObserver {
    attempt: Attempt,
    observations: &'static ObservationStorage,
}
impl oer_trace::Observer for StationObserver {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        crate::measurements::RADIO.observe(&event);
        self.observations.event(
            event,
            self.attempt.request,
            EPOCH.load(core::sync::atomic::Ordering::Relaxed),
            &Publisher,
        );
    }
}
pub(super) struct Publisher;
impl oer_ieee80211_control::collector::Publish for Publisher {
    fn publish<M: oer_hil_protocol::Message>(&self, request: RequestIdentity, message: &M) {
        emit(request, message);
    }
}
fn counters(observations: &ObservationStorage) -> oer_ieee80211_control::StationCounters {
    observations.counters(
        EPOCH.load(core::sync::atomic::Ordering::Relaxed),
        COUNTERS.count::<oer_ieee80211_instrumentation::TxQueueCollision>(),
        u32::try_from(
            oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes(),
        )
        .unwrap_or_else(|_| radio::failure(wifi::InitializationError::Control)),
    )
}

static CAPTURES: Controls<(RequestIdentity, oer_hil_agent::capture::Request)> = Controls::new();
async fn capture(observations: &'static ObservationStorage) {
    loop {
        let (request, command) = CAPTURES.receive().await;
        command.serve(observations).publish(&CONSOLE, request).await;
    }
}
