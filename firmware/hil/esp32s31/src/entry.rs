#[cfg(not(any(
    feature = "boot-smoke",
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "wifi-station"
)))]
compile_error!("select boot-smoke, system-watchdog, system-panic-reset or wifi-station");
#[cfg(all(feature = "system-watchdog", feature = "boot-smoke"))]
compile_error!("system-watchdog requires an exclusive image");
#[cfg(all(
    feature = "system-panic-reset",
    any(
        feature = "panic-hook",
        feature = "boot-smoke",
        feature = "system-watchdog"
    )
))]
compile_error!("system-panic-reset requires an exclusive image with the product panic entry");

#[cfg(all(
    feature = "wifi-station",
    any(
        feature = "boot-smoke",
        feature = "system-watchdog",
        feature = "system-panic-reset"
    )
))]
compile_error!("wifi-station requires an exclusive image");

use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};
use oer_diagnostics::Note;
use oer_esp32s31_compose::Esp32s31;

#[cfg(feature = "boot-smoke")]
use embassy_time::{Duration, Timer};
use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::FromCpu0Irq, system_timer::SystimerAlarm0,
};
#[cfg(feature = "wifi-station")]
use oer_esp32s31_hal::peripherals::{
    software_interrupt::FromCpu2Irq, system_timer::SystimerAlarm1,
};
use oer_espressif_executor::Executor;
#[cfg(feature = "boot-smoke")]
use BootSmokeTasks as SingleExecutorTasks;
#[cfg(feature = "system-watchdog")]
use system::console::WatchdogTasks as SingleExecutorTasks;
#[cfg(feature = "system-panic-reset")]
use system::panic_reset::PanicResetTasks as SingleExecutorTasks;

#[cfg(feature = "wifi-station")]
mod phy_pilot;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    #[cfg(not(feature = "wifi-station"))]
    /// Wakes the core-0 executor.
    wake: ExecutorWake = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    #[cfg(feature = "wifi-station")]
    wake: ExecutorWake = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    #[cfg(not(feature = "wifi-station"))]
    /// The Embassy time driver's alarm (system timer comparator 0).
    alarm: TimeAlarm = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<SingleExecutorTasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    #[cfg(feature = "wifi-station")]
    /// The application executor's alarm (system timer comparator 1).
    alarm: TimeAlarm = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<wifi_station::AppTasks, 1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    #[cfg(feature = "wifi-station")]
    /// The radio executor's alarm (system timer comparator 0).
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<wifi_station::RadioTasks, 0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    #[cfg(feature = "wifi-station")]
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    #[cfg(feature = "wifi-station")]
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<wifi_station::RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    #[cfg(feature = "wifi-station")]
    /// The radio handler (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = wifi_station::irq::WifiPart => crate::wifi_station::irq::on_wifi, line 21, context radio, hart 0, Hardware;
    #[cfg(not(feature = "wifi-station"))]
    /// The USB Serial/JTAG console: it raises the console endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 18, context app_wake, hart 0, Hardware;
    #[cfg(feature = "wifi-station")]
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 42, context app_wake, hart 0, Hardware;
}

#[cfg(feature = "boot-smoke")]
mod boot_smoke_console;
#[cfg(feature = "boot-smoke")]
mod boot_smoke_rearm;
mod exception;
mod fatal;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "wifi-station"
))]
mod services;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "wifi-station"
))]
mod system;
#[cfg(any(
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "wifi-station"
))]
mod transport;
mod usb;
#[cfg(feature = "wifi-station")]
mod wifi_station;

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

#[cfg(feature = "boot-smoke")]
oer_espressif_executor::tasks! {
    /// The boot smoke's task.
    BootSmokeTasks: Executor<Esp32s31, 0> {
        boot_smoke(console: boot_smoke_console::BootSmokeConsole) = boot_smoke;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
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

#[cfg(feature = "panic-hook")]
/// The image's record of a panic, which the platform's panic entry calls
/// after its own record, with the other hart stopped, before it resets the
/// chip (feature `panic-hook`): it records the post-mortem fault and the machine state,
/// formatting nothing, so the panic path stays a leaf in every stack
/// bound. The next boot reports what it recorded.
#[inline(always)]
fn panic_hook(
    claim: &oer_espressif_staged_runtime::retained_fault::Claim<'_>,
    details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>,
    stop: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    #[cfg(feature = "system-watchdog")]
    system::postmortem::record_panic(claim, details);
    #[cfg(not(feature = "system-watchdog"))]
    let _ = details;
    let stage = 0;
    fatal::record_panic(claim, stage, stop);
}

/// The image's entropy owner.
#[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
pub(crate) type Rng =
    oer_esp32s31_hal::peripherals::entropy::Entropy<oer_embassy_time::EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
#[cfg(feature = "panic-hook")]
oer_espressif_staged_runtime::entry!(
    main,
    oer_esp32s31_hal::platform::Port,
    hook = panic_hook,
    exception = exception::capture
);
#[cfg(not(feature = "panic-hook"))]
oer_espressif_staged_runtime::entry!(
    main,
    oer_esp32s31_hal::platform::Port,
    exception = exception::capture
);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    #[cfg(feature = "wifi-station")]
    wifi_station::start(handoff);
    #[cfg(not(feature = "wifi-station"))]
    system_main(handoff)
}

#[cfg(not(feature = "wifi-station"))]
fn system_main(
    handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>,
) -> ! {
    let prepared = initialize_runtime();
    let ((timer, software, watchdogs, entropy), _psram, mut adopted) = finish_startup(|| {
        prepared.adopt(
            handoff,
            INTERRUPT_TABLE,
            oer_esp32s31_board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |runtime| {
                #[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
                system::read_reset_causes(runtime.reset_cause);
                #[cfg(not(any(feature = "system-watchdog", feature = "system-panic-reset")))]
                let _reset_causes = oer_esp32s31_compose::reset_causes::read(runtime.reset_cause);
                fatal::report_previous();
                // Before anything records a checkpoint over the previous boot's record.
                #[cfg(feature = "system-watchdog")]
                system::postmortem::begin();
                // Snapshot before boot evidence; acknowledge only after complete output.
                #[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
                system::take_platform_panic();
                (
                    runtime.system_timer,
                    runtime.software_interrupts,
                    runtime.watchdogs,
                    runtime.entropy,
                )
            },
        )
    })
    .into_parts();
    #[cfg(not(any(feature = "system-watchdog", feature = "system-panic-reset")))]
    let _entropy = entropy;
    exception::report_stack_guard();

    let Some(interrupts) = Interrupts::take() else {
        fail(fatal::Failure::InterruptTable);
    };
    let _timer_ready = oer_espressif_executor::init::<Esp32s31, 0, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let console = usb::open(adopted.router(), interrupts.console_usb);
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => crate::fail(crate::fatal::Failure::from_boot(error)),
    };
    let wake = (interrupts.wake, software.irq0);
    let rtc_watchdog = watchdogs.rtc;

    #[cfg(feature = "system-panic-reset")]
    system::panic_reset::start(
        executor,
        &hart,
        &mut router,
        wake,
        console,
        crate::Rng::new(entropy, oer_embassy_time::EmbassyClock),
        rtc_watchdog,
    );

    #[cfg(feature = "system-watchdog")]
    system::console::start(
        executor,
        &hart,
        &mut router,
        wake,
        console,
        crate::Rng::new(entropy, oer_embassy_time::EmbassyClock),
        watchdogs.main1,
        rtc_watchdog,
    );

    #[cfg(feature = "boot-smoke")]
    match executor
        .run(&hart, &mut router, wake.0, wake.1, (), move |spawner| {
            let console = boot_smoke_console::BootSmokeConsole::new(console);
            if BootSmokeTasks::boot_smoke(spawner, console).is_err()
                || BootSmokeTasks::rtc_watchdog(spawner, rtc_watchdog).is_err()
            {
                fail(fatal::Failure::TaskAllocation);
            }
        })
        .unwrap_or_else(|failure| failure.reset()) {};
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
    match watchdog
        .feed_every(&oer_embassy_time::EmbassyClock, period)
        .await {}
}

#[cfg(feature = "boot-smoke")]
async fn boot_smoke(mut console: boot_smoke_console::BootSmokeConsole) {
    console.embassy_started().await;
    Timer::after(Duration::from_millis(50)).await;
    let rearm = boot_smoke_rearm::run(async |measurement: &boot_smoke_rearm::Measurement| {
        console.measurement(measurement).await;
    })
    .await;
    if let Err(failure) = rearm {
        console.rearm_failed(&failure).await;
        core::future::pending::<()>().await;
    }
    console.timer_passed().await;
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
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
            bottom >= PROFILE_DATA_START
                && top <= PROFILE_DATA_END
                && top.checked_sub(bottom)
                    == oer_esp32s31_layout::LAYOUT
                        .task_stack_bytes
                        .get(hart)
                        .copied()
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

/// Halt the image unless the TRNG produces health-tested entropy.
#[cfg(feature = "system-watchdog")]
pub(crate) fn check_entropy_source(entropy: &Rng) {
    if entropy.status().is_healthy() {
        transport::CONSOLE.note_immediately(&Note::entropy_pass());
    } else {
        fail(fatal::Failure::EntropySource);
    }
}

/// Writes `note` before the console opens.
fn early(note: &Note<'_>) {
    usb::early(note);
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
