use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};
use oer_diagnostics::Note;
use oer_esp32s31_compose::Esp32s31;

use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::FromCpu0Irq, system_timer::SystimerAlarm0,
};
use oer_espressif_executor::Executor;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; }
    /// Wakes the core-0 executor.
    wake: ExecutorWake = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The Embassy time driver's alarm (system timer comparator 0).
    alarm: TimeAlarm = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<ImageTasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// The USB Serial/JTAG console: it raises the console endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 18, context app_wake, hart 0, Hardware;
}

mod aes;
mod exception;
mod fatal;
mod services;
mod system;
mod transport;
mod usb;

oer_espressif_executor::tasks! {
    /// The image's tasks: the console, the AES service and the RTC
    /// watchdog's feed.
    ImageTasks: Executor<Esp32s31, 0> {
        console(usb: transport::Usb, boot: u64, evidence: system::Evidence) = console;
        aes_service(engine: aes::Engine) = aes::serve;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

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
    let ((timer, software, watchdogs, entropy, runtime_aes, evidence), _psram, mut adopted) =
        finish_startup(|| {
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
                    (
                        runtime.system_timer,
                        runtime.software_interrupts,
                        runtime.watchdogs,
                        runtime.entropy,
                        runtime.aes,
                        evidence,
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
    let engine = oer_services_aes::Engine::new(oer_esp32s31_compose::crypto::key_unwrap(
        runtime_aes,
        silent,
    ));
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
    let (usb, _pads) = usb::open(adopted.router(), interrupts.console_usb);
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(fatal::Failure::from_boot(error)),
    };
    let mut entropy = oer_esp32s31_hal::peripherals::entropy::Entropy::new(
        entropy,
        oer_embassy_time::EmbassyClock,
    );
    let boot = u64::from_le_bytes(entropy.random_bytes()).max(1);
    transport::CONSOLE.start(boot);
    let rtc_watchdog = watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if ImageTasks::console(spawner, usb, boot, evidence).is_err()
                    || ImageTasks::aes_service(spawner, engine).is_err()
                    || ImageTasks::rtc_watchdog(spawner, rtc_watchdog).is_err()
                {
                    fail(fatal::Failure::TaskAllocation);
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Serve the host until the chip resets.
async fn console(usb: transport::Usb, boot: u64, evidence: system::Evidence) {
    transport::serve(
        usb,
        boot,
        services::KEYS,
        services::MAXIMUM_PAYLOAD_BYTES,
        evidence,
        aes::queue,
    )
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
    match watchdog
        .feed_every(&oer_embassy_time::EmbassyClock, period)
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
