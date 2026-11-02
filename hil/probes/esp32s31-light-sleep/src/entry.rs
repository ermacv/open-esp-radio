use oer_chips_esp32s31_board as board;
use oer_chips_esp32s31_compose::Esp32s31;
use oer_chips_esp32s31_hal::peripherals::sleep::SleepRegisters;
use oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu0Irq;
use oer_chips_esp32s31_hal::peripherals::system_timer::SystimerAlarm0;
use oer_chips_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_chips_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_chips_esp32s31_hal::platform::Port;
use oer_chips_esp32s31_hal::root::Root;
use oer_chips_family_executor::Executor;
use oer_chips_family_power::sleep::{self, Outcome, SleepClocks, SleepError, SleepRequest, Slept};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock as _, Duration, Timer as _};

use crate::calc::{Line, nominal_ticks, rate_hz};

oer_chips_family_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; }
    /// The boot hart's executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_chips_family_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The boot hart's executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_chips_family_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
}

oer_chips_family_executor::tasks! {
    /// The boot hart's tasks.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        measure(sleep: SleepRegisters) = measure;
    }
}

/// One sleep: its clock gates and its length at the nominal slow rate.
struct Case {
    clocks: SleepClocks,
    millis: u64,
}

/// The cases in order: running clocks first, the gated case last, since
/// gated clocks may stop the USB console.
const CASES: [Case; 5] = [
    Case { clocks: SleepClocks::Running, millis: 10 },
    Case { clocks: SleepClocks::Running, millis: 100 },
    Case { clocks: SleepClocks::Running, millis: 1_000 },
    Case { clocks: SleepClocks::Running, millis: 100 },
    Case { clocks: SleepClocks::Gated, millis: 100 },
];

/// The wait before the first case and between two cases: the console
/// drains and the watchdog's task runs.
const PAUSE: Duration = Duration::from_millis(200);

oer_chips_family_staged_runtime::entry!(main, Port);

fn main(handoff: oer_chips_family_staged_runtime::Handoff<Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_portable_diagnostics::name!("root-taken"; 32));
    };
    let ((timer, software, watchdogs, sleep), _psram, mut adopted) =
        match oer_chips_family_staged_runtime::adoption::run::<oer_chips_esp32s31_compose::Esp32s31, _, _>(
            handoff,
            root.into_runtime(),
            INTERRUPT_TABLE,
            board::BootstrapPsram,
            oer_chips_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    hardware.sleep,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_portable_diagnostics::name!("interrupt-table"; 32));
    };
    let _timers = oer_chips_family_executor::init::<Esp32s31, 0, _, _, _>(
        oer_chips_family_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let watchdog = watchdogs.rtc;
    text(b"OER-PROBE light-sleep ready\r\n");
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    match executor
        .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err()
                || Tasks::measure(spawner, sleep).is_err()
            {
                fail(oer_portable_diagnostics::name!("task-start"; 32));
            }
        })
        .unwrap_or_else(|failure| failure.reset())
    {}
}

/// The wait after boot before the first line that matters: the host's
/// console attaches to the re-enumerated USB device.
const ATTACH: Duration = Duration::from_secs(3);

/// The LP-timer target check's length.
const CHECK_MILLIS: u64 = 100;

/// The counter twice, then the target check without sleep, then every case
/// in order, and the verdict: the target fired awake and every case woke
/// after at least its length and before twice it. No case sleeps unless
/// the target fired awake: the RTC watchdog pauses in sleep, so a sleep
/// whose wake never comes would not end.
async fn measure(mut sleep: SleepRegisters) {
    if EmbassyClock.wait_for(ATTACH).await.is_err() {
        fail(oer_portable_diagnostics::name!("attach-time"; 32));
    }
    let complete = counter_runs(&mut sleep).await && target_fires(&mut sleep);
    let mut passed = complete;
    if complete {
        for (index, case) in CASES.iter().enumerate() {
            if EmbassyClock.wait_for(PAUSE).await.is_err() {
                fail(oer_portable_diagnostics::name!("pause-time"; 32));
            }
            passed &= run_case(&mut sleep, index, case);
        }
    }
    if EmbassyClock.wait_for(PAUSE).await.is_err() {
        fail(oer_portable_diagnostics::name!("pause-time"; 32));
    }
    text(if passed {
        b"OER-RESULT: pass light-sleep\r\n"
    } else {
        b"OER-RESULT: fail light-sleep\r\n"
    });
}

/// Two counter readings a pause apart, with the system time between them.
async fn counter_runs(sleep_registers: &mut SleepRegisters) -> bool {
    let first = sleep::now(sleep_registers);
    let before = EmbassyClock.now().as_micros();
    if EmbassyClock.wait_for(PAUSE).await.is_err() {
        fail(oer_portable_diagnostics::name!("pause-time"; 32));
    }
    let second = sleep::now(sleep_registers);
    let timer_us = EmbassyClock.now().as_micros().saturating_sub(before);
    let mut line = Line::new();
    line.text("OER-PROBE light-sleep counter");
    let runs = match (first, second) {
        (Ok(first), Ok(second)) => {
            let ticks = second.saturating_since(first);
            line.text(" first=")
                .number(first.get())
                .text(" second=")
                .number(second.get())
                .text(" timer_us=")
                .number(timer_us)
                .text(" rtc_hz=")
                .number(rate_hz(ticks, timer_us).unwrap_or(0));
            ticks > 0
        }
        _ => {
            line.text(" error=counter");
            false
        }
    };
    text(line.end());
    runs
}

/// The first target armed [`CHECK_MILLIS`] ahead and waited for awake.
fn target_fires(sleep_registers: &mut SleepRegisters) -> bool {
    let Some(ticks) = nominal_ticks(CHECK_MILLIS) else {
        return false;
    };
    let result = oer_portable_riscv::interrupt_free(|| {
        let start = sleep::now(sleep_registers)?;
        let at = start.checked_add(ticks).ok_or(SleepError::Bound)?;
        let give_up_after = at.checked_add(ticks).ok_or(SleepError::Bound)?;
        let before = EmbassyClock.now().as_micros();
        let fired = sleep::check_timer(sleep_registers, at, give_up_after)?;
        let timer_us = EmbassyClock.now().as_micros().saturating_sub(before);
        Ok::<(u64, u64), SleepError>((fired.saturating_since(start), timer_us))
    });
    let mut line = Line::new();
    line.text("OER-PROBE light-sleep target ms=").number(CHECK_MILLIS);
    let fires = match result {
        Ok((rtc, timer_us)) => {
            line.text(" fired rtc_ticks=")
                .number(rtc)
                .text(" timer_us=")
                .number(timer_us);
            rtc >= ticks && rtc < ticks.saturating_mul(2)
        }
        Err(error) => {
            line.text(" error=").text(error_name(error));
            false
        }
    };
    text(line.end());
    fires
}

fn error_name(error: SleepError) -> &'static str {
    match error {
        SleepError::Bound => "bound",
        SleepError::TooSoon { .. } => "too-soon",
        SleepError::Counter(_) => "counter",
        SleepError::NoOutcome { .. } => "no-outcome",
    }
}

/// One sleep with interrupts masked; its line, and whether it woke within
/// its bounds.
fn run_case(sleep_registers: &mut SleepRegisters, index: usize, case: &Case) -> bool {
    let clocks = match case.clocks {
        SleepClocks::Running => "running",
        SleepClocks::Gated => "gated",
    };
    text(
        Line::new()
            .text("OER-PROBE light-sleep case=")
            .number(index as u64)
            .text(" clocks=")
            .text(clocks)
            .text(" ms=")
            .number(case.millis)
            .text(" start")
            .end(),
    );
    let Some(ticks) = nominal_ticks(case.millis) else {
        return false;
    };
    let result = oer_portable_riscv::interrupt_free(|| {
        let start = sleep::now(sleep_registers)?;
        let wake_at = start.checked_add(ticks).ok_or(SleepError::Bound)?;
        let give_up_after = wake_at.checked_add(ticks).ok_or(SleepError::Bound)?;
        let before = EmbassyClock.now().as_micros();
        let slept = sleep::sleep_until(
            sleep_registers,
            SleepRequest {
                wake_at,
                give_up_after,
                clocks: case.clocks,
            },
        )?;
        let after = EmbassyClock.now().as_micros();
        Ok::<(Slept, u64), SleepError>((slept, after.saturating_sub(before)))
    });
    let mut line = Line::new();
    line.text("OER-PROBE light-sleep case=").number(index as u64);
    let pass = match result {
        Ok((slept, timer_us)) => {
            let rtc = slept.resumed.saturating_since(slept.requested);
            let (outcome, causes, woke) = match slept.outcome {
                Outcome::Woke { causes } => ("woke", causes, true),
                Outcome::Rejected { causes } => ("rejected", causes, false),
            };
            line.text(" outcome=")
                .text(outcome)
                .text(" causes=")
                .hex(causes)
                .text(" rtc_ticks=")
                .number(rtc)
                .text(" timer_us=")
                .number(timer_us)
                .text(" rtc_hz=")
                .number(rate_hz(rtc, timer_us).unwrap_or(0));
            woke && rtc >= ticks && rtc < ticks.saturating_mul(2)
        }
        Err(error) => {
            line.text(" error=").text(error_name(error));
            false
        }
    };
    text(line.end());
    pass
}

async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Write `bytes` through the early writer, within its bounded budget.
fn text(bytes: &[u8]) {
    let _written = usb_serial_jtag::early_text(bytes);
}

/// Retain and report a failed boot step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_portable_diagnostics::Name<B>) -> ! {
    oer_chips_family_staged_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = usb_serial_jtag::fatal_note(&oer_portable_diagnostics::Note::runtime_fail(reason));
    })
}

oer_chips_family_executor::time_driver!(Esp32s31);
