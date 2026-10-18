#![no_main]
#![no_std]
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

use core::{arch::asm, ffi::CStr, ptr};

#[cfg(feature = "boot-smoke")]
use embassy_time::{Duration, Timer};
use esp_hal::timer::{OneShotTimer, timg::TimerGroup};
use oer_espressif_executor_embassy::Executor;
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the core-0 Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (TIMG0 timer 0).
    alarm: TimeAlarm = TG0_T0_LEVEL => oer_espressif_executor_embassy::timer_interrupt, Priority1, ProCpu;
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
const INTERNAL_SRAM_START: u32 = 0x2f00_0000;
const INTERNAL_SRAM_END: u32 = 0x2f07_afc0;

const PROFILE_CODE_START: u32 = 0x5001_0000;
const PROFILE_CODE_END: u32 = 0x5100_0000;
const PROFILE_DATA_START: u32 = 0x5000_0000;
const PROFILE_DATA_END: u32 = 0x5100_0000;
const PROFILE_NAME: &core::ffi::CStr = c"psram-code-psram-data-psram-stack";

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static mut INITIALIZED_DATA: u32 = DATA_SENTINEL;
static mut BSS_PROBE: u32 = 0;

#[used]
#[unsafe(link_section = ".isr.rodata.profile_probe")]
static ISR_RODATA_PROBE: u32 = 0x4953_5231;
#[used]
#[unsafe(link_section = ".critical.data.profile_probe")]
static mut CRITICAL_DATA_PROBE: u32 = 0x4352_5431;
#[used]
#[unsafe(link_section = ".dma.data.profile_probe")]
static mut DMA_DATA_PROBE: u32 = 0x444d_4131;
oer_memory::zeroed_static! {
    #[used]
    static mut DMA_BSS_PROBE: u32 =
        zeroed in ".dma.bss.profile_probe";
}

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;

    static __runtime_image_start: u8;
    static __runtime_payload_end: u8;
    static __runtime_bss_start: u8;
    static __runtime_bss_end: u8;
    static __runtime_data_load_start: u8;
    static __runtime_data_start: u8;
    static __runtime_data_end: u8;
    static __runtime_data_bss_start: u8;
    static __runtime_data_bss_end: u8;
    static __runtime_isr_start: u8;
    static __runtime_isr_end: u8;
    static __runtime_dma_data_start: u8;
    static __runtime_dma_data_end: u8;
    static __runtime_dma_bss_start: u8;
    static __runtime_dma_bss_end: u8;
    static __runtime_cpu0_irq_stack_bottom: u8;
    static __runtime_cpu0_irq_stack_top: u8;
    static __runtime_cpu1_irq_stack_bottom: u8;
    static __runtime_cpu1_irq_stack_top: u8;
    static _stack_end: u8;
    static _stack_start: u8;
}

use oer_espressif_staged_runtime as _;

/// The image's record of a panic, which the platform's panic entry calls
/// after its own record and before it resets the chip (feature `panic-hook`):
/// it records the post-mortem fault and the machine
/// state, formatting nothing, so the panic path stays a leaf in every stack
/// bound. The next boot reports what it recorded.
#[cfg(feature = "panic-hook")]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".flash.critical.text.panic_hook")]
fn oer_platform_panic_hook(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    #[cfg(feature = "system-watchdog")]
    system::postmortem::record_panic(details);
    #[cfg(not(feature = "system-watchdog"))]
    let _ = details;
    let stage = 0;
    fatal::record_panic(stage);
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    unsafe { ets_install_usb_printf() };
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
    let _psram =
        unsafe { oer_esp32s31_platform_board::adopt_psram(peripherals.PSRAM, INTERRUPT_TABLE) };
    exception::install_stack_guard(ptr::addr_of!(_stack_end) as usize);

    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    oer_espressif_executor_embassy::init(OneShotTimer::new(timer_group.timer0), interrupts.alarm);

    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupt::executor0(
        peripherals.FROM_CPU_INTR0,
    )));

    // SAFETY: bootstrap intentionally hands MIE over clear, and timer and
    // software wake interrupt ownership is complete at this point.
    unsafe { oer_espressif_staged_runtime::enable_interrupts_after_handoff() };

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
    let image_start = symbol(ptr::addr_of!(__runtime_image_start));
    let payload_end = symbol(ptr::addr_of!(__runtime_payload_end));
    let bss_start = symbol(ptr::addr_of!(__runtime_bss_start));
    let bss_end = symbol(ptr::addr_of!(__runtime_bss_end));
    let data_load_start = symbol(ptr::addr_of!(__runtime_data_load_start));
    let data_start = symbol(ptr::addr_of!(__runtime_data_start));
    let data_end = symbol(ptr::addr_of!(__runtime_data_end));
    let runtime_bss_start = symbol(ptr::addr_of!(__runtime_data_bss_start));
    let runtime_bss_end = symbol(ptr::addr_of!(__runtime_data_bss_end));
    let isr_start = symbol(ptr::addr_of!(__runtime_isr_start));
    let isr_end = symbol(ptr::addr_of!(__runtime_isr_end));
    let dma_data_start = symbol(ptr::addr_of!(__runtime_dma_data_start));
    let dma_data_end = symbol(ptr::addr_of!(__runtime_dma_data_end));
    let dma_bss_start = symbol(ptr::addr_of!(__runtime_dma_bss_start));
    let dma_bss_end = symbol(ptr::addr_of!(__runtime_dma_bss_end));
    let stack_bottom = symbol(ptr::addr_of!(_stack_end));
    let stack_top = symbol(ptr::addr_of!(_stack_start));
    let stack = current_stack_pointer();

    let interrupt_stacks_valid = {
        let cpu0_bottom = symbol(ptr::addr_of!(__runtime_cpu0_irq_stack_bottom));
        let cpu0_top = symbol(ptr::addr_of!(__runtime_cpu0_irq_stack_top));
        let cpu1_bottom = symbol(ptr::addr_of!(__runtime_cpu1_irq_stack_bottom));
        let cpu1_top = symbol(ptr::addr_of!(__runtime_cpu1_irq_stack_top));
        range_in_internal_sram(cpu0_bottom, cpu0_top)
            && range_in_internal_sram(cpu1_bottom, cpu1_top)
            && cpu0_top - cpu0_bottom == oer_esp32s31_platform_layout::LAYOUT.irq_stack_bytes
            && cpu1_top - cpu1_bottom == oer_esp32s31_platform_layout::LAYOUT.irq_stack_bytes
    };
    let task_stack_valid = stack_bottom >= PROFILE_DATA_START
        && stack_top <= PROFILE_DATA_END
        && stack_top - stack_bottom == oer_esp32s31_platform_layout::LAYOUT.cpu0_task_stack_bytes;

    let initialized_data = unsafe { ptr::addr_of!(INITIALIZED_DATA).read_volatile() };
    let bss_probe = unsafe { ptr::addr_of!(BSS_PROBE).read_volatile() };
    let isr_probe = unsafe { ptr::addr_of!(ISR_RODATA_PROBE).read_volatile() };
    let critical_probe = unsafe { ptr::addr_of!(CRITICAL_DATA_PROBE).read_volatile() };
    let dma_probe = unsafe { ptr::addr_of!(DMA_DATA_PROBE).read_volatile() };
    let dma_bss_probe = unsafe { ptr::addr_of!(DMA_BSS_PROBE).read_volatile() };

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

fn symbol(address: *const u8) -> u32 {
    address as usize as u32
}

fn current_stack_pointer() -> u32 {
    let value: usize;
    unsafe { asm!("mv {value}, sp", value = out(reg) value, options(nomem, nostack)) };
    value as u32
}

/// Halt the image unless the TRNG produces health-tested entropy.
#[cfg(feature = "system-watchdog")]
pub(crate) fn check_entropy_source() {
    if oer_esp32s31_soc_esp_hal::entropy::source_status().is_healthy() {
        print(c"OPEN_RADIO_HIL entropy=PASS\r\n");
    } else {
        fail(c"OPEN_RADIO_HIL runtime=FAIL reason=entropy-source\r\n");
    }
}

fn print(message: &'static CStr) {
    unsafe { ets_printf(message.as_ptr()) };
}

fn fail(message: &'static CStr) -> ! {
    print(message);
    halt()
}

fn halt() -> ! {
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
