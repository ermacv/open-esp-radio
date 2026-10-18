//! ESP32-C5 HIL runtime.
//!
//! The platform bootstrap stages this runtime into PSRAM and enters it at
//! `_runtime_start` (`oer-espressif-staged-runtime`), which calls
//! [`runtime_main`]. It runs the scheduler-free Embassy executor of
//! `oer-espressif-executor-embassy` on the image's interrupt table. Each image
//! selects one feature: `boot-smoke` proves the boot path with text lines,
//! `system-watchdog` serves the HIL protocol with the SoC deadline watchdog
//! and the reset-retained post-mortem.

#![no_std]
#![no_main]

#[cfg(not(any(feature = "boot-smoke", feature = "system-watchdog")))]
compile_error!("select the boot-smoke or system-watchdog image");
#[cfg(all(feature = "boot-smoke", feature = "system-watchdog"))]
compile_error!("boot-smoke and system-watchdog are separate images");

use esp_hal::{
    interrupt::software::SoftwareInterrupt,
    timer::{OneShotTimer, timg::TimerGroup},
};
use oer_espressif_executor_embassy::Executor;
use static_cell::StaticCell;

#[cfg(feature = "boot-smoke")]
mod boot_smoke;
#[cfg(feature = "system-watchdog")]
mod system;

oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (TIMG0 timer 0).
    alarm: TimeAlarm = TG0_T0_LEVEL => oer_espressif_executor_embassy::timer_interrupt, Priority1, ProCpu;
    #[cfg(feature = "system-watchdog")]
    /// The USB Serial/JTAG console, to esp-hal's async driver.
    console_usb: ConsoleUsb = USB_DEVICE => esp_hal::usb::usb_serial_jtag::handle_async_interrupt, Priority1, ProCpu;
}

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();

/// The image's record of a panic, which the platform's panic entry calls
/// after its own record and before it resets the chip: the post-mortem's
/// fault, formatting nothing. The next boot reports it.
#[cfg(feature = "system-watchdog")]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".flash.critical.text.panic_hook")]
fn oer_platform_panic_hook(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    system::postmortem::record_panic(details);
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    // What the previous boot left is taken before anything records.
    #[cfg(feature = "system-watchdog")]
    {
        system::postmortem::begin();
        system::take_platform_panic();
    }
    #[cfg(feature = "boot-smoke")]
    let _ = oer_espressif_staged_runtime::panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: the stage-two entry runs after the board bootstrap, with global
    // interrupts disabled and the PSRAM mapping intact.
    let _psram =
        unsafe { oer_esp32c5_platform_board::adopt_psram(peripherals.PSRAM, INTERRUPT_TABLE) };

    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    let timers = TimerGroup::new(peripherals.TIMG0);
    oer_espressif_executor_embassy::init(OneShotTimer::new(timers.timer0), interrupts.alarm);
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    #[cfg(feature = "boot-smoke")]
    let console = boot_smoke::start(peripherals.USB_DEVICE);
    #[cfg(feature = "system-watchdog")]
    let console = system::console::Console::new(
        peripherals.USB_DEVICE,
        interrupts.console_usb,
        peripherals.TIMG1,
    );
    // SAFETY: the timer and executor handlers are bound, and the staged
    // handoff has kept MIE clear since `adopt_psram`.
    unsafe { oer_espressif_staged_runtime::enable_interrupts_after_handoff() };
    executor.run(interrupts.wake, move |spawner| {
        #[cfg(feature = "boot-smoke")]
        spawner.spawn(boot_smoke::run(console).expect("the smoke task's storage is free once"));
        #[cfg(feature = "system-watchdog")]
        spawner
            .spawn(system::console::run(console).expect("the console task's storage is free once"));
    })
}
