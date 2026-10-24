//! ESP32-C5 HIL runtime.
//!
//! The platform bootstrap stages this runtime into PSRAM and enters it at
//! `_runtime_start` (`oer-espressif-staged-runtime`), which calls
//! [`runtime_main`]. It runs the scheduler-free static executor of
//! `oer-espressif-executor` on the image's interrupt table. Each image
//! selects one feature: `boot-smoke` proves the boot path with text lines,
//! `system-watchdog` serves the HIL protocol with the SoC deadline watchdog
//! and the reset-retained post-mortem.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

#[cfg(not(any(feature = "boot-smoke", feature = "system-watchdog")))]
compile_error!("select the boot-smoke or system-watchdog image");
#[cfg(all(feature = "boot-smoke", feature = "system-watchdog"))]
compile_error!("boot-smoke and system-watchdog are separate images");

use esp_hal::interrupt::software::SoftwareInterrupt;
use oer_espressif_executor::Executor;

#[cfg(feature = "boot-smoke")]
mod boot_smoke;
#[cfg(feature = "boot-smoke")]
mod boot_smoke_rearm;
#[cfg(feature = "system-watchdog")]
mod system;
mod usb;

oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu, Hardware;
    /// The Embassy time driver's alarm (system timer comparator 0).
    alarm: TimeAlarm = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority1, ProCpu, Hardware;
    /// The USB Serial/JTAG console: it raises the console endpoint's signal.
    console_usb: ConsoleUsb = USB_DEVICE => crate::usb::interrupt, Priority1, ProCpu, Hardware;
}

oer_espressif_executor::tasks! {
    /// The image's one task.
    AppTasks: Executor<0> {
        #[cfg(feature = "boot-smoke")]
        smoke(console: boot_smoke::Console) = boot_smoke::run;
        #[cfg(feature = "system-watchdog")]
        serve(console: system::console::Console) = system::console::run;
    }
}

// The image's record of a panic, which the platform's panic entry calls
// after its own record and before it resets the chip: the post-mortem's
// fault, formatting nothing. The next boot reports it.
#[cfg(feature = "system-watchdog")]
oer_espressif_staged_runtime::panic_hook!(system::postmortem::record_panic);

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // What the previous boot left is taken before anything records.
    #[cfg(feature = "system-watchdog")]
    {
        system::postmortem::begin();
        system::take_platform_panic();
    }
    #[cfg(feature = "boot-smoke")]
    let _ = oer_espressif_staged_runtime::panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (_psram, adopted) = handoff.adopt(
        INTERRUPT_TABLE,
        oer_esp32c5_board::BootstrapPsram(peripherals.PSRAM),
    );

    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    oer_espressif_executor::init::<0, _>(peripherals.SYSTIMER, interrupts.alarm);
    let executor = Executor::<0>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR0));
    #[cfg(feature = "boot-smoke")]
    let console = boot_smoke::start(peripherals.USB_DEVICE, interrupts.console_usb);
    #[cfg(feature = "system-watchdog")]
    let console = system::console::Console::new(
        oer_esp32c5_esp_hal_soc::handoff::take_usb_serial_jtag(peripherals.USB_DEVICE),
        interrupts.console_usb,
        oer_esp32c5_esp_hal_soc::handoff::take_watchdogs(
            peripherals.TIMG0,
            peripherals.TIMG1,
            peripherals.LP_WDT,
        ),
        oer_esp32c5_hal::platform::Entropy::new(
            oer_esp32c5_esp_hal_soc::handoff::take_entropy(peripherals.RNG, peripherals.LP_PERI),
            oer_embassy_time::EmbassyClock,
        ),
        oer_esp32c5_hal::platform::ResetCauses::read(
            oer_esp32c5_esp_hal_soc::handoff::take_reset_cause(peripherals.LP_CLKRST),
        ),
    );
    adopted.enable_interrupts();
    executor.run(interrupts.wake, move |spawner| {
        #[cfg(feature = "boot-smoke")]
        AppTasks::smoke(spawner, console).expect("the smoke task's first start");
        #[cfg(feature = "system-watchdog")]
        AppTasks::serve(spawner, console).expect("the console task's first start");
    })
}
