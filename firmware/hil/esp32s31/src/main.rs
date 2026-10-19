#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![recursion_limit = "256"]
#[cfg(not(any(
    feature = "boot-smoke",
    feature = "system-watchdog",
    feature = "system-panic-reset"
)))]
compile_error!("select boot-smoke, system-watchdog or system-panic-reset");
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

use core::ffi::CStr;
use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "boot-smoke")]
use embassy_time::{Duration, Timer};
use oer_espressif_executor::Executor;
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the core-0 Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (system timer comparator 0).
    alarm: TimeAlarm = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority1, ProCpu;
    #[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
    /// The USB Serial/JTAG console, to esp-hal's async driver.
    console_usb: ConsoleUsb = USB_DEVICE => esp_hal::usb::usb_serial_jtag::handle_async_interrupt, Priority1, ProCpu;
}

#[cfg(feature = "boot-smoke")]
mod boot_smoke_console;
mod exception;
mod fatal;
#[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
mod image_features;
mod software_interrupt;
#[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
mod system;
#[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
mod transport;

const DATA_SENTINEL: u32 = 0x5353_31d2;
const INTERNAL_SRAM_START: u32 = oer_esp32s31_layout::LAYOUT.sram.origin;
const INTERNAL_SRAM_END: u32 = oer_esp32s31_layout::LAYOUT.sram.end();

const PROFILE_CODE_START: u32 = oer_esp32s31_layout::LAYOUT.runtime_psram().origin;
const PROFILE_CODE_END: u32 = oer_esp32s31_layout::LAYOUT.psram.end();
const PROFILE_DATA_START: u32 = oer_esp32s31_layout::LAYOUT.psram.origin;
const PROFILE_DATA_END: u32 = oer_esp32s31_layout::LAYOUT.psram.end();
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-psram-data-psram-stack";

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
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
oer_espressif_staged_runtime::panic_hook!(panic_hook);

#[cfg(feature = "panic-hook")]
oer_espressif_staged_runtime::sram_fn! {
    in critical_text "panic_hook";
    /// The image's record of a panic, which the platform's panic entry calls
    /// after its own record and before it resets the chip (feature
    /// `panic-hook`): it records the post-mortem fault and the machine state,
    /// formatting nothing, so the panic path stays a leaf in every stack
    /// bound. The next boot reports what it recorded.
    #[inline(always)]
    fn panic_hook(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
        #[cfg(feature = "system-watchdog")]
        system::postmortem::record_panic(details);
        #[cfg(not(feature = "system-watchdog"))]
        let _ = details;
        let stage = 0;
        fatal::record_panic(stage);
    }
}

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_board::rom::install_usb_printf();
    print(c"OPEN_RADIO_HIL runtime=START profile=");
    print(PROFILE_NAME);
    print(c"\r\n");
    // Before anything can fail and overwrite it.
    fatal::report_previous();

    validate_runtime_layout();

    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    // Before anything records a checkpoint over the previous boot's record.
    #[cfg(feature = "system-watchdog")]
    system::postmortem::begin();
    // Before any boot evidence is served: taking it clears the record.
    #[cfg(any(feature = "system-watchdog", feature = "system-panic-reset"))]
    system::take_platform_panic();
    // The bootstrap configured PSRAM before entering this separately linked
    // runtime. `esp_hal::init()` cannot carry process-local mapping metadata
    // across that ELF boundary, so stage two explicitly adopts the live
    // hardware mapping without reinitializing the PSRAM device or MMU.
    let (_psram, adopted) = handoff.adopt(
        INTERRUPT_TABLE,
        oer_esp32s31_board::BootstrapPsram(peripherals.PSRAM),
    );
    exception::report_stack_guard();

    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    oer_espressif_executor::init::<0, _>(peripherals.SYSTIMER, interrupts.alarm);

    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupt::executor0(
        peripherals.FROM_CPU_INTR0,
    )));

    adopted.enable_interrupts();

    #[cfg(feature = "system-panic-reset")]
    system::panic_reset::start(
        executor,
        interrupts.wake,
        transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb),
        peripherals.RNG,
    );

    #[cfg(feature = "system-watchdog")]
    system::console::start(
        executor,
        interrupts.wake,
        transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb),
        peripherals.RNG,
        peripherals.TIMG1,
    );

    #[cfg(feature = "boot-smoke")]
    executor.run(interrupts.wake, move |spawner| {
        let Ok(task) = boot_smoke(boot_smoke_console::BootSmokeConsole::new(
            peripherals.USB_DEVICE,
        )) else {
            fail(c"OPEN_RADIO_HIL runtime=FAIL reason=task-allocation\r\n");
        };
        spawner.spawn(task);
    });
}

#[cfg(feature = "boot-smoke")]
#[embassy_executor::task]
async fn boot_smoke(mut console: boot_smoke_console::BootSmokeConsole) {
    console.embassy_started();
    Timer::after(Duration::from_millis(50)).await;
    console.timer_passed();
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
    let [stack_bottom, stack_top] = bounds(&sections.task_stack);
    let stack = word(oer_espressif_staged_runtime::trap::stack_pointer());

    let interrupt_stacks_valid = {
        let [cpu0_bottom, cpu0_top] = bounds(&sections.irq_stacks[0]);
        let [cpu1_bottom, cpu1_top] = bounds(&sections.irq_stacks[1]);
        range_in_internal_sram(cpu0_bottom, cpu0_top)
            && range_in_internal_sram(cpu1_bottom, cpu1_top)
            && cpu0_top - cpu0_bottom == oer_esp32s31_layout::LAYOUT.irq_stack_bytes
            && cpu1_top - cpu1_bottom == oer_esp32s31_layout::LAYOUT.irq_stack_bytes
    };
    let task_stack_valid = stack_bottom >= PROFILE_DATA_START
        && stack_top <= PROFILE_DATA_END
        && stack_top - stack_bottom == oer_esp32s31_layout::LAYOUT.cpu0_task_stack_bytes;

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
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=layout\r\n");
    }
    print(c"OPEN_RADIO_HIL placement=PASS isr=SRAM dma_probes=SRAM task_stack=PSRAM irq_stack=SRAM\r\n");
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
pub(crate) fn check_entropy_source() {
    if oer_espressif_esp_hal_soc::entropy::source_status().is_healthy() {
        print(c"OPEN_RADIO_HIL entropy=PASS\r\n");
    } else {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=entropy-source\r\n");
    }
}

fn print(message: &'static CStr) {
    oer_esp32s31_board::rom::print(message);
}

fn fail(message: &'static CStr) -> ! {
    print(message);
    halt()
}

fn halt() -> ! {
    oer_espressif_staged_runtime::idle()
}
