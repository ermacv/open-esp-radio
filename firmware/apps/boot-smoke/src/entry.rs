use oer_esp32s31_compose::Esp32s31;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::peripherals::{software_interrupt::FromCpu0Irq, system_timer::SystimerAlarm0};
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_esp32s31_hal::peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_espressif_executor::Executor;
use oer_espressif_staged_runtime::diagnostics::{self, BootSmokePassed, Console};
use oer_espressif_staged_runtime::panic;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; }
    /// The executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Esp32s31, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The executor's one task.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::peek_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|error| fail(error.name()));
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
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
    let _ = diagnostics.event(&BootSmokePassed);
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<Esp32s31, 0, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime.watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if Tasks::rtc_watchdog(spawner, watchdog).is_err() {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|_, _path| {
        let _note = usb_serial_jtag::fatal_note(&oer_diagnostics::Note::runtime_fail(reason));
    })
}
