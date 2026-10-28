mod future;
pub(crate) mod irq;
mod network;
mod observation;
mod radio;
mod report;
mod station;
use crate::services::StationRequests as Requests;
use oer_esp32s31_compose::Esp32s31;
use crate::transport::CONSOLE;
use oer_hil_protocol::{
    RequestIdentity,
    wifi::{
        StationLink, StationLinkState, StationRole, StationServiceEnd, StationServiceEnded,
        StationStart, StopStation,
    },
};
use oer_ieee80211_rsn::{PmkDerivation, nonce::SNonceSource, sae::SaePassword};
use oer_time::Clock;

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::platform::FromCpu3Irq;
use oer_esp32s31_hal::power::ChipClocks;
use oer_esp32s31_hal::watchdog::{ArmedRtcWatchdog, RwdtRegisters};
use oer_esp32s31_ieee80211_driver::irq::WifiIrqPart;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Doorbell, InterruptExecutor};

use oer_memory::{Placed, StaticPlace};
use static_cell::{ConstStaticCell, StaticCell};
static NONCES: ConstStaticCell<StaticPlace<SNonceSource>> =
    ConstStaticCell::new(StaticPlace::new());

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

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    pub(crate) RadioTasks: InterruptExecutor<Esp32s31, 2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(boot: radio::Boot, channel: radio::DriverChannel) = radio::drive;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<Esp32s31, 0> {
        console(work: oer_memory::PlacedFuture<'static, ConsoleWork>) = console;
        station_task(work: oer_memory::PlacedFuture<'static, StationWork>) = station_task;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = crate::feed_rtc_watchdog;
    }
}

static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
        NoopPhyTargetObserver,
        crate::phy_pilot::Client,
    >,
    EmbassyClock,
>;

pub(crate) fn start(
    handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>,
) -> ! {
    crate::early(&oer_diagnostics::Note::runtime_start(crate::PROFILE_NAME));
    crate::fatal::report_previous();
    crate::validate_runtime_layout();
    crate::system::take_platform_panic();
    let Some(root) = oer_esp32s31_hal::root::Root::take() else {
        crate::fail(crate::fatal::Failure::RootTaken);
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::RootInitialization));
    oer_esp32s31_compose::wifi_fault::report_previous();
    let (_psram, mut adopted) = match handoff.adopt(
        crate::INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
    ) {
        Ok(adopted) => adopted,
        Err(error) => crate::fail(crate::fatal::Failure::from_boot(error)),
    };
    crate::exception::report_stack_guard();
    let config = CONFIG;
    let Some(interrupts) = crate::Interrupts::take() else {
        crate::fail(crate::fatal::Failure::InterruptTable);
    };
    crate::system::read_reset_causes(runtime.reset_cause);
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(runtime.efuse);
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let ap = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let (domain, partitions) = runtime
        .radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_esp32s31_compose::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_esp32s31_compose::phy_fault::reset(fault)
        });
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        park()
    };
    let phy_pilot = crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), interrupts.phy);
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(adopted.router(), interrupts.alarm, timer.alarm1)
        .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    // The radio handler's part holds both banks and the doorbell's lowering
    // half; its binding makes the doorbell's ringing half.
    let (line, lower) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            WifiIrqPart::new(runtime.wifi_interrupts, software.irq3, irq::Lower(lower)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let bell = irq::ImageDoorbell(bell);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let mut rng = crate::Rng::new(runtime.entropy, EmbassyClock);
    if !rng.status().is_healthy() {
        crate::fail(crate::fatal::Failure::EntropySource);
    }
    let nonces = Placed::init(NONCES.take().uninit(), |place| {
        place.write(SNonceSource::from_entropy(|seed| rng.fill(seed)))
    });
    let mut boot_bytes = [0; 8];
    rng.fill(&mut boot_bytes);
    let seed = u64::from_le_bytes(boot_bytes);
    let boot = seed.max(1);
    let mut tx_bytes = [0; 4];
    rng.fill(&mut tx_bytes);
    let entropy = u32::from_le_bytes(tx_bytes);
    CONSOLE.start(boot);
    let (driver_channel, frontend_channel) = radio::channels();
    if RadioTasks::radio_actor(&radio, actor).is_err() {
        crate::fail(crate::fatal::Failure::TaskAllocation);
    }
    if RadioTasks::driver(
        &radio,
        radio::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng,
            station,
            ap,
            config,
            bound,
            bell,
            entropy,
        },
        driver_channel,
    )
    .is_err()
    {
        crate::fail(crate::fatal::Failure::TaskAllocation);
    }
    let (usb, _pads) = crate::usb::open(adopted.router(), interrupts.console_usb);
    let executor = crate::Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => crate::fail(crate::fatal::Failure::from_boot(error)),
    };
    let watchdog = runtime.watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            urgent,
            move |spawner| {
                let console_work = CONSOLE_WORK.take().future(|| console_future(usb, boot));
                if AppTasks::console(spawner, console_work).is_err() {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
                let station_work = STATION_WORK
                    .take()
                    .future(|| station_work_future(frontend_channel, station, seed, nonces));
                if AppTasks::station_task(spawner, station_work).is_err() {
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
fn park() -> ! {
    oer_espressif_staged_runtime::idle()
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
            .expect("station epoch exhausted; reset required");
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
static PROBES: Controls<(RequestIdentity, oer_hil_protocol::wifi::ProbeStation)> = Controls::new();
static STOPS: Controls<RequestIdentity> = Controls::new();
type ConsoleWork = impl core::future::Future<Output = ()>;
static CONSOLE_WORK: ConstStaticCell<StaticPlace<ConsoleWork>> =
    ConstStaticCell::new(StaticPlace::new());
#[define_opaque(ConsoleWork)]
fn console_future(usb: crate::transport::Usb, boot: u64) -> ConsoleWork {
    console_work(usb, boot)
}
/// The console's task: its work lives in its own static place.
async fn console(work: oer_memory::PlacedFuture<'static, ConsoleWork>) {
    work.await;
}
async fn console_work(usb: crate::transport::Usb, boot: u64) {
    use oer_hil_protocol::base::{Accepted, RejectReason, Rejected};
    let mut configured = false;
    let mut stopping = false;
    crate::transport::serve(
        usb,
        boot,
        crate::services::STATION_KEYS,
        0,
        move |request, command: Requests| {
        let reason = match command {
            Requests::Counters(_) => {
                emit(request, &observation::counters());
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
            Requests::Probe(probe) => {
                if let Some(reason) = probe.refusal(role(configured, stopping)) {
                    Some(reason)
                } else if PROBES.try_send((request, probe)).is_err() {
                    Some(RejectReason::Busy)
                } else {
                    None
                }
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
    })
    .await
}

/// The station's task: its work lives in its own static place.
async fn station_task(work: oer_memory::PlacedFuture<'static, StationWork>) {
    work.await;
}
type StationWork = impl core::future::Future<Output = ()>;
static STATION_WORK: ConstStaticCell<StaticPlace<StationWork>> =
    ConstStaticCell::new(StaticPlace::new());
#[define_opaque(StationWork)]
fn station_work_future(
    channel: radio::FrontendChannel,
    address: [u8; 6],
    seed: u64,
    nonces: Placed<'static, SNonceSource>,
) -> StationWork {
    station_work(channel, address, seed, nonces)
}
async fn station_work(
    channel: radio::FrontendChannel,
    address: [u8; 6],
    seed: u64,
    nonces: Placed<'static, SNonceSource>,
) {
    let (request, mut start) = STARTS.receive().await;
    let attempt = Attempt { request };
    attempt.advance();
    let ssid = heapless::Vec::from_slice(start.credentials.ssid()).expect("decoded SSID fits");
    let password =
        SaePassword::new(start.credentials.passphrase()).expect("decoded personal password fits");
    let mut derive = PmkDerivation::new(start.credentials.passphrase(), start.credentials.ssid())
        .expect("decoded personal credentials fit");
    // All retained copies now have their own zeroizing owner. Erase the
    // received command before the first cooperative cancellation point.
    start.credentials.clear_passphrase();
    drop(start);
    let begin = EmbassyClock.now();
    let pmk = loop {
        if let Some(pmk) = derive.step() {
            break pmk;
        }
        embassy_futures::yield_now().await;
    };
    let elapsed = EmbassyClock
        .now()
        .as_micros()
        .saturating_sub(begin.as_micros());
    CONSOLE
        .note_reliably(&oer_diagnostics::Note::station_pmk(elapsed))
        .await;
    radio::serve(
        channel,
        station::Body {
            address,
            seed,
            ssid,
            password,
            pmk,
            nonces,
            attempt,
        },
    )
    .await;
}

use oer_ieee80211_arbiter::{
    ArbiterPolicy, ArbiterPolicyError, CheckedArbiterPolicy, NoReservation,
};
use oer_time::Duration;
pub fn arbiter(
    publication_bound: Duration,
) -> Result<CheckedArbiterPolicy<NoReservation>, ArbiterPolicyError> {
    let mut policy = ArbiterPolicy::DEFAULT;
    let absence = publication_bound
        .as_micros()
        .checked_add(policy.retune.as_micros())
        .and_then(|one| one.checked_mul(2))
        .and_then(|moves| moves.checked_add(policy.min_listen.as_micros()))
        .ok_or(ArbiterPolicyError::AbsenceBudgetOverflow)?;
    policy.absence = Duration::from_micros(absence);
    policy.checked(publication_bound)
}

// Keep the bounded wire-record constructor out of control/worker poll frames.
#[inline(never)]
fn emit<M: oer_hil_protocol::Message>(request: RequestIdentity, body: &M) {
    CONSOLE.publish(request.session_id, request.request_id, body);
}
