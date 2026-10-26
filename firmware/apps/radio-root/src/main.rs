#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

//! Takes the reset root and requests and releases each platform clock once
//! through the chip's platform clock owner. The host renders its typed
//! verdict as `OER-RESULT: pass radio-root`. It powers no RF, PHY or MAC.
//! Its one executor feeds the RTC watchdog the root armed.

use oer_diagnostics::radio_root::{self, RadioRoot};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::platform::{FromCpu0Irq, SystimerAlarm0};
use oer_esp32s31_hal::power::{PlatformClock, PlatformClockError, PlatformClockProvider as _};
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::usb_serial_jtag;
use oer_esp32s31_hal::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_espressif_executor::Executor;
use oer_espressif_staged_runtime::diagnostics::{self, Console};
use oer_espressif_staged_runtime::panic;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; }
    priority: low_to_high;
    /// The executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The executor's one task.
    Tasks: Executor<0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let Some(root) = Root::take() else {
        fail("root-taken");
    };
    let runtime = root.into_runtime();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    // The executor is not running yet: the early writer waits for room
    // within its budget and counts what it drops.
    let mut diagnostics = Console::new(|bytes| {
        if usb_serial_jtag::early_text(bytes) {
            Ok(())
        } else {
            Err(())
        }
    });
    if let Some(record) = previous {
        let _ = diagnostics::previous_panic(&mut diagnostics, &record);
    }
    // The radio root stays held, untouched, until reset.
    let _radio = runtime.radio;
    let clocks = runtime.clocks;
    let mut passed = true;
    for clock in PlatformClock::ALL {
        match clocks.acquire(clock) {
            Ok(guard) => {
                guard.release();
                let _ = diagnostics.event(&RadioRoot::ClockReleased(wire(clock)));
            }
            Err(PlatformClockError) => {
                passed = false;
                let _ = diagnostics.event(&RadioRoot::ClockRefused(wire(clock)));
            }
        }
    }
    let _ = diagnostics.event(if passed {
        &RadioRoot::Passed
    } else {
        &RadioRoot::ClockFailed
    });
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupt-table");
    };
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<0, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    );
    let software = runtime.software_interrupts;
    let executor = Executor::<0>::new(software.line0);
    let mut router = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime.watchdogs.rtc;
    executor.run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err() {
                fail("task-start");
            }
        },
    )
}

/// The clock's wire identity in the radio-root record.
const fn wire(clock: PlatformClock) -> radio_root::PlatformClock {
    match clock {
        PlatformClock::Pll160m => radio_root::PlatformClock::Pll160m,
        PlatformClock::AnalogI2cMaster => radio_root::PlatformClock::AnalogI2cMaster,
        PlatformClock::Mpll => radio_root::PlatformClock::Mpll,
        PlatformClock::ModemCoexistence => radio_root::PlatformClock::ModemCoexistence,
        PlatformClock::ModemLowPowerTimer => radio_root::PlatformClock::ModemLowPowerTimer,
    }
}

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail(reason: &'static str) -> ! {
    oer_espressif_staged_runtime::fatal_halt(|_path| {
        let _note = usb_serial_jtag::fatal_note(&oer_diagnostics::Note::runtime_fail(reason));
    })
}
