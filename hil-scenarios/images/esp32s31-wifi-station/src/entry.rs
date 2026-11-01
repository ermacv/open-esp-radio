use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};
use oer_diagnostics::Note;
use oer_esp32s31_compose::Esp32s31;

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu1Irq, FromCpu2Irq, FromCpu3Irq},
    system_timer::{SystimerAlarm0, SystimerAlarm1, SystimerAlarm2},
    timg0_alarm::Timg0AlarmInterrupt,
    timg1_alarm::Timg1AlarmInterrupt,
};
use oer_esp32s31_wifi::{self as wifi, InterruptTypes as _};
use oer_espressif_executor::{AlarmGrant, Doorbell, DoorbellLine, Executor, InterruptExecutor};

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; profile: 4; network_executor: 2; network_alarm: 3; }
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    /// The SAR/PWDET conversion steps between radio executor rounds.
    phy: PhyToken = Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<station::RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio executor's alarm (system timer comparator 0).
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<station::RadioTasks, 0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    /// The radio handler (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = station::irq::WifiPart => crate::station::irq::on_wifi, line 21, context radio, hart 0, Hardware;
    /// The PC sampler (TIMG1 T0), above every other level so that it sees
    /// what it interrupts; it ticks only while a run samples.
    sampler: SamplerToken = Timg1AlarmInterrupt => load::sampler::interrupt, line 22, context profile, hart 0, Hardware;
    /// Wakes the application executor.
    wake: ExecutorWake = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    /// The application executor's alarm (system timer comparator 1).
    alarm: TimeAlarm = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    /// The USB Serial/JTAG console: it raises the console endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 42, context app_wake, hart 0, Hardware;
    /// Hart 1's network executor (P2 of hart 1), polled in its handler.
    network: NetworkToken = FromCpu1Irq => oer_espressif_executor::interrupt_handler::<network::NetworkTasks, FromCpu1Irq>, line 16, context network_executor, hart 1, Executor;
    /// The network executor's alarm (system timer comparator 2), above it
    /// on hart 1; it only acknowledges and pends.
    network_alarm: NetworkAlarmToken = SystimerAlarm2 => oer_espressif_executor::alarm_interrupt::<network::NetworkTasks, 2, SystimerAlarm2>, line 17, context network_alarm, hart 1, Hardware;
}

mod exception;
mod fatal;
mod network;
mod peers;
mod phy_pilot;
mod services;
mod station;
mod system;
mod transport;
mod usb;

oer_espressif_executor::tasks! {
    /// The application executor's tasks: the console, the station's work,
    /// the relay of the load server on hart 1 and the RTC watchdog's feed.
    AppTasks: Executor<Esp32s31, 0> {
        @observer: load::cpu::Observer::<{ load::cpu::APPLICATION }>;
        console(usb: transport::Usb, boot: u64, evidence: system::Evidence, requests: Requests) = console;
        station_work(work: oer_memory::PlacedFuture<'static, station::Work>) = station::run;
        relay(relay: load::relay::Relay) = load::relay::run;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

/// What hart 1 takes at its launch: its executor, which owns `FROM_CPU1`'s
/// line, that line's interrupt part and table token, its executor's timer
/// grant and its network task's ends.
struct Hart1Parts {
    executor: peers::NetworkExecutor,
    irq: FromCpu1Irq,
    token: NetworkToken,
    alarm: AlarmGrant<Esp32s31, 1, NetworkAlarmToken>,
    network: network::Inputs,
}

oer_espressif_staged_runtime::secondary_entry!(1, HART1: oer_esp32s31_hal::platform::Port, Hart1Parts => hart1_main);

/// The hart `secondary_entry!` above roots and the manifest's `harts`
/// declares: the image links its task stack at the stack policy's launch
/// size, not at the chip's zero default for a hart no image starts.
const SECONDARY_HART: oer_esp32s31_hal::platform::Hart =
    oer_esp32s31_hal::platform::Hart::constant::<1>();

const DATA_SENTINEL: u32 = 0x5353_31d2;
const INTERNAL_SRAM_START: u32 = oer_esp32s31_layout::LAYOUT.sram.origin();
const INTERNAL_SRAM_END: u32 = oer_esp32s31_layout::LAYOUT.sram.end();

const PROFILE_CODE_START: u32 = oer_esp32s31_layout::LAYOUT
    .runtime_psram()
    .expect("runtime PSRAM fits")
    .origin();
const PROFILE_CODE_END: u32 = oer_esp32s31_layout::LAYOUT.psram.end();
const PROFILE_DATA_START: u32 = oer_esp32s31_layout::LAYOUT.psram.origin();
const PROFILE_DATA_END: u32 = oer_esp32s31_layout::LAYOUT.psram.end();
const PROFILE_NAME: oer_diagnostics::Name<64> =
    oer_diagnostics::name!("psram-code-psram-data-psram-stack"; 64);

/// Layout probes: each lives in the section it checks and reads back its
/// initializer once the runtime has loaded or zeroed that section.
static INITIALIZED_DATA: AtomicU32 = AtomicU32::new(DATA_SENTINEL);
static BSS_PROBE: AtomicU32 = AtomicU32::new(0);
oer_espressif_staged_runtime::placed_static! {
    #[used]
    static ISR_RODATA_PROBE: u32 = 0x4953_5231, in isr_rodata "profile_probe";
    #[used]
    static CRITICAL_DATA_PROBE: AtomicU32 = AtomicU32::new(0x4352_5431), in critical_data "profile_probe";
    #[used]
    static DMA_DATA_PROBE: AtomicU32 = AtomicU32::new(0x444d_4131), in dma_data "profile_probe";
}
oer_memory::zeroed_static! {
    #[used]
    static DMA_BSS_PROBE: AtomicU32 =
        zeroed in ".dma.bss.profile_probe";
}

use oer_espressif_staged_runtime as _;

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(
    main,
    oer_esp32s31_hal::platform::Port,
    exception = exception::capture
);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let prepared = initialize_runtime();
    let (
        (
            aes,
            efuse,
            radio,
            clocks,
            timg0_alarm,
            timg1_alarm,
            cache_counters,
            system_timer,
            software,
            wifi_interrupts,
            entropy,
            watchdogs,
            evidence,
            hart1,
        ),
        _psram,
        mut adopted,
    ) = finish_startup(|| {
        prepared.adopt(
            handoff,
            INTERRUPT_TABLE,
            oer_esp32s31_board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |runtime| {
                // Before boot evidence is served; only complete output
                // acknowledges the previous boot's platform panic.
                let evidence = system::Evidence::read(runtime.reset_cause);
                fatal::report_previous();
                oer_esp32s31_compose::wifi_fault::report_previous();
                (
                    runtime.aes,
                    runtime.efuse,
                    runtime.radio,
                    runtime.clocks,
                    runtime.timg0_alarm,
                    runtime.timg1_alarm,
                    runtime.cache_counters,
                    runtime.system_timer,
                    runtime.software_interrupts,
                    runtime.wifi_interrupts,
                    runtime.entropy,
                    runtime.watchdogs,
                    evidence,
                    runtime.hart1,
                )
            },
        )
    })
    .into_parts();
    exception::report_stack_guard();

    let Some(interrupts) = Interrupts::take() else {
        fail(fatal::Failure::InterruptTable);
    };
    let silent = adopted
        .router()
        .silent::<oer_esp32s31_hal::peripherals::aes::AesInterrupt>()
        .unwrap_or_else(|_| fail(fatal::Failure::InterruptTable));
    let engine = oer_esp32s31_compose::crypto::key_unwrap(aes, silent);
    let factory = oer_esp32s31_compose::platform::DeviceIdentity::read(efuse);
    let address = factory.station_address();
    let ap = factory.ap_address();

    // The shared RF actor and its SAR/PWDET binding; the actor calibrates at
    // the station's first registration, after the join.
    let resources =
        wifi::RadioResources::new::<station::radio::binding::Binding>(radio, clocks, &factory);
    let pilot = phy_pilot::init(timg0_alarm, adopted.router(), interrupts.phy);
    let (actor, hardware) = resources.start(pilot);
    let actor = station::ACTOR
        .try_init(actor)
        .unwrap_or_else(|| fail(fatal::Failure::Storage));
    // The radio executor writes the system timer on comparator 0; the
    // application executor publishes its deadlines to it for comparator 1,
    // and hart 1's network executor for comparator 2, whose alarm hart 1
    // routes itself.
    let mut timers = oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: system_timer.comparators,
            counter: system_timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        system_timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(
        &mut timers,
        adopted.router(),
        interrupts.alarm,
        system_timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    let network_alarm = oer_espressif_executor::grant::<Esp32s31, 1, _>(
        &mut timers,
        interrupts.network_alarm,
        system_timer.alarm2,
    );

    // The load server on hart 1 reaches the boot hart's console, sampler
    // and counters through the relay; the station hands hart 1 its network
    // end through its ends of the start.
    let sampler = load::sampler::init(timg1_alarm, adopted.router(), interrupts.sampler);
    let (relay, hart0) = load::relay::split(sampler, cache_counters);
    let (load_sender, load_receiver) = load::serve::requests();
    let (network, network_inputs) = network::split(load_receiver, hart0);
    // Hart 1 starts before any executor: it installs its own stacks and
    // exception path, then its own interrupt context, timer and executor;
    // a failure after its release resets.
    let Ok(Some(mut cores)) = oer_esp32s31_hal::cores::Cores::take() else {
        fail(fatal::Failure::Cores);
    };
    let _hart1 = oer_esp32s31_compose::secondary::launch_hart1(
        &mut cores,
        hart1,
        evidence.causes(),
        &timers,
        &HART1,
        Hart1Parts {
            executor: InterruptExecutor::<Esp32s31, 1>::new(software.line1),
            irq: software.irq1,
            token: interrupts.network,
            alarm: network_alarm,
            network: network_inputs,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    // The radio handler's part holds both banks and the doorbell's lowering
    // half; its binding makes the doorbell's ringing half.
    let (line, lower) = DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            wifi::Interrupts::part(wifi_interrupts, software.irq3, wifi::Lower(lower)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    if station::RadioTasks::radio_actor(&radio, actor).is_err() {
        fail(fatal::Failure::TaskAllocation);
    }

    let mut rng = oer_esp32s31_compose::platform::Entropy::new(entropy, EmbassyClock);
    if !rng.is_healthy() {
        fail(fatal::Failure::EntropySource);
    }
    let nonces = wifi::NonceSource::from_entropy(|seed| rng.fill(seed));
    let mut words = [0; 20];
    rng.fill(&mut words);
    let [
        b0,
        b1,
        b2,
        b3,
        b4,
        b5,
        b6,
        b7,
        s0,
        s1,
        s2,
        s3,
        s4,
        s5,
        s6,
        s7,
        e0,
        e1,
        e2,
        e3,
    ] = words;
    let boot = u64::from_le_bytes([b0, b1, b2, b3, b4, b5, b6, b7]).max(1);
    let seed = u64::from_le_bytes([s0, s1, s2, s3, s4, s5, s6, s7]);
    let entropy = u32::from_le_bytes([e0, e1, e2, e3]);
    transport::CONSOLE.start(boot);
    let dma = wifi::DmaResources::claim()
        .unwrap_or_else(|error| station::radio::failure(wifi::InitializationError::Dma(error)));
    let (sender, receiver) = station::requests();
    let inputs = station::Inputs {
        requests: receiver,
        deferred: station::Deferred {
            engine,
            hardware,
            rng,
            address,
            ap,
            bound,
            bell: wifi::Bell(bell),
            entropy,
            dma,
        },
        radio,
        network,
        seed,
        nonces,
    };

    let (usb, _pads) = usb::open(adopted.router(), interrupts.console_usb);
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(fatal::Failure::from_boot(error)),
    };
    let rtc_watchdog = watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            urgent,
            move |spawner| {
                let requests = Requests {
                    station: sender,
                    load: load_sender,
                };
                if AppTasks::console(spawner, usb, boot, evidence, requests).is_err()
                    || AppTasks::station_work(spawner, station::place(inputs)).is_err()
                    || AppTasks::relay(spawner, relay).is_err()
                    || AppTasks::rtc_watchdog(spawner, rtc_watchdog).is_err()
                {
                    fail(fatal::Failure::TaskAllocation);
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Hart 1's main, on its own stacks with its interrupt stack and exception
/// path installed: make its own interrupt context, route its executor's
/// alarm, start its network executor on `FROM_CPU1` with the network task,
/// enable its interrupts and wait for them.
fn hart1_main(
    handoff: oer_espressif_staged_runtime::SecondaryHandoff<oer_esp32s31_hal::platform::Port, 1>,
    parts: Hart1Parts,
) -> ! {
    let Hart1Parts {
        executor,
        irq,
        token,
        alarm,
        network,
    } = parts;
    let mut adopted = match handoff.adopt(INTERRUPT_TABLE) {
        Ok(adopted) => adopted,
        Err(error) => fail(fatal::Failure::from_boot(error)),
    };
    let (hart, router) = adopted.parts();
    alarm
        .bind(hart, router)
        .unwrap_or_else(|failure| failure.reset());
    let spawner = executor
        .start::<network::NetworkTasks, _, _, 1, _>(hart, router, token, irq, ())
        .unwrap_or_else(|failure| failure.reset());
    if network::NetworkTasks::network(&spawner, network).is_err() {
        fail(fatal::Failure::TaskAllocation);
    }
    match adopted.enable_interrupts() {
        Ok(_router_and_hart) => oer_espressif_staged_runtime::idle(),
        Err(error) => fail(fatal::Failure::from_boot(error)),
    }
}

/// The console's ends of the services' request queues.
struct Requests {
    station: station::Sender,
    load: load::serve::Sender,
}

/// Serve the host until the chip resets; the station's requests go to its
/// work, the load requests to the load server beside it.
async fn console(usb: transport::Usb, boot: u64, evidence: system::Evidence, requests: Requests) {
    use oer_hil_agent::base::Either;
    let serve = core::pin::pin!(transport::serve(
        usb,
        boot,
        services::KEYS,
        services::MAXIMUM_PAYLOAD_BYTES,
        evidence,
        move |identity,
              request: Either<oer_services_wifi_station::Request, oer_services_load::Request>| {
            match request {
                Either::First(request) => station::queue(&requests.station, identity, request),
                Either::Second(request) => load::serve::queue(&requests.load, identity, request),
            }
        },
    ));
    load::parts::Polled {
        part: load::parts::Part::Console,
        future: serve,
    }
    .await
}

/// Install the console and arm the watchdogs before observing boot evidence.
fn initialize_runtime() -> oer_espressif_staged_runtime::adoption::Prepared<Esp32s31> {
    oer_esp32s31_compose::platform::prepare().unwrap_or_else(|error| {
        fail(match error {
            oer_esp32s31_compose::platform::PrepareError::Claimed => fatal::Failure::RootTaken,
            oer_esp32s31_compose::platform::PrepareError::Initialization(_) => {
                fatal::Failure::RootInitialization
            }
        })
    })
}

/// Reset/retained evidence precedes adoption; ordinary startup checks follow it.
fn finish_startup<R>(
    adopt: impl FnOnce() -> Result<R, oer_espressif_staged_runtime::BootError>,
) -> R {
    startup::initialize(adopt, || {
        early(&Note::runtime_start(PROFILE_NAME));
        validate_runtime_layout();
    })
    .unwrap_or_else(|error| fail(fatal::Failure::from_boot(error)))
}

/// Feed the RTC watchdog for good: the image resets when its executor stops
/// polling.
pub(crate) async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    let feed = core::pin::pin!(watchdog.feed_every(&oer_embassy_time::EmbassyClock, period));
    match (load::parts::Polled {
        part: load::parts::Part::RtcWatchdog,
        future: feed,
    })
    .await {}
}

fn validate_runtime_layout() {
    let sections = oer_espressif_staged_runtime::layout::sections();
    let [image_start, payload_end] = [sections.image.start, sections.image.end].map(word);
    let data_load_start = word(sections.data_load_start);
    let [data_start, data_end] = bounds(&sections.data);
    let [bss_start, bss_end] = bounds(&sections.bss);
    let [runtime_bss_start, runtime_bss_end] = bounds(&sections.data_bss);
    let [isr_start, isr_end] = bounds(&sections.isr);
    let [dma_data_start, dma_data_end] = bounds(&sections.dma_data);
    let [dma_bss_start, dma_bss_end] = bounds(&sections.dma_bss);
    let [stack_bottom, stack_top] =
        bounds(oer_esp32s31_hal::platform::BOOT_HART.at(&sections.task_stacks));
    let stack = word(oer_riscv::stack_pointer());

    let interrupt_stacks_valid = sections.irq_stacks.iter().all(|range| {
        let [bottom, top] = bounds(range);
        range_in_internal_sram(bottom, top)
            && top
                .checked_sub(bottom)
                .is_some_and(|bytes| bytes <= oer_esp32s31_layout::LAYOUT.irq_stack_bytes)
    });
    let task_stack_valid = sections
        .task_stacks
        .iter()
        .enumerate()
        .all(|(hart, range)| {
            let [bottom, top] = bounds(range);
            let bytes = top.checked_sub(bottom);
            // A started secondary hart's stack is sized by the image's
            // link; every other hart keeps the chip's default allocation.
            let sized = if hart == SECONDARY_HART.get() {
                bytes.is_some_and(|bytes| bytes != 0 && bytes.is_multiple_of(16))
            } else {
                bytes
                    == oer_esp32s31_layout::LAYOUT
                        .task_stack_bytes
                        .get(hart)
                        .copied()
            };
            bottom >= PROFILE_DATA_START && top <= PROFILE_DATA_END && sized
        });

    let initialized_data = INITIALIZED_DATA.load(Ordering::Relaxed);
    let bss_probe = BSS_PROBE.load(Ordering::Relaxed);
    let isr_probe = oer_memory::read_volatile(&ISR_RODATA_PROBE);
    let critical_probe = CRITICAL_DATA_PROBE.load(Ordering::Relaxed);
    let dma_probe = DMA_DATA_PROBE.load(Ordering::Relaxed);
    let dma_bss_probe = DMA_BSS_PROBE.load(Ordering::Relaxed);

    if image_start != PROFILE_CODE_START
        || payload_end <= image_start
        || payload_end > PROFILE_CODE_END
        || data_load_start < PROFILE_CODE_START
        || data_load_start >= payload_end
        || data_end < data_start
        || runtime_bss_end < runtime_bss_start
        || bss_end < bss_start
        || !(PROFILE_DATA_START..PROFILE_DATA_END).contains(&data_start)
        || !(PROFILE_DATA_START..=PROFILE_DATA_END).contains(&runtime_bss_end)
        || !range_in_internal_sram(isr_start, isr_end)
        || !range_in_internal_sram(dma_data_start, dma_data_end)
        || !range_in_internal_sram(dma_bss_start, dma_bss_end)
        || !interrupt_stacks_valid
        || !task_stack_valid
        || !(stack_bottom..stack_top).contains(&stack)
        || !stack.is_multiple_of(16)
        || initialized_data != DATA_SENTINEL
        || bss_probe != 0
        || isr_probe != 0x4953_5231
        || critical_probe != 0x4352_5431
        || dma_probe != 0x444d_4131
        || dma_bss_probe != 0
    {
        fail(fatal::Failure::Layout);
    }
    early(&Note::placement_pass());
}

fn range_in_internal_sram(start: u32, end: u32) -> bool {
    start >= INTERNAL_SRAM_START && end >= start && end <= INTERNAL_SRAM_END
}

fn word(address: usize) -> u32 {
    address as u32
}

fn bounds(range: &Range<usize>) -> [u32; 2] {
    [range.start, range.end].map(word)
}

/// Writes `note` before the console opens.
fn early(note: &Note<'_>) {
    let _written = usb::early(note);
}

/// Retain every failed boot check before best-effort output and explicit reset.
fn fail(failure: fatal::Failure) -> ! {
    use oer_esp32s31_hal::{cores, peripherals::usb_serial_jtag::fatal_note};
    oer_espressif_staged_runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|claim, path| {
        fatal::record_failure(claim, failure);
        let _note = fatal_note(&Note::runtime_fail(failure.name()));
        let cores::AwaitWatchdog { reason: _ } = cores::software_reset(path);
    })
}

oer_espressif_executor::time_driver!(Esp32s31);
