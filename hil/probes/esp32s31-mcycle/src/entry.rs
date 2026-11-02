use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Poll;

use oer_portable_embassy_time::{EmbassyClock, WaitUntil};
use oer_chips_esp32s31_board as board;
use oer_chips_esp32s31_compose::Esp32s31;
use oer_chips_esp32s31_hal::cores::Cores;
use oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu0Irq;
use oer_chips_esp32s31_hal::peripherals::system_timer::SystimerAlarm0;
use oer_chips_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_chips_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_chips_esp32s31_hal::platform::Port;
use oer_chips_esp32s31_hal::root::Root;
use oer_chips_family_executor::Executor;
use oer_chips_family_staged_runtime::SecondaryHandoff;
use oer_portable_time::{Clock as _, Duration, Timer as _};

use crate::calc::{Line, offset, per_10ms};

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
        measure() = measure;
    }
}

/// Reads per read-cost case.
const READS: u32 = 256;
/// The span of a rate case, in timer microseconds.
const RATE_SPAN: u64 = 10_000;
/// The span of a wait-for-interrupt case.
const WFI_SPAN: Duration = Duration::from_millis(1);
/// Runs per wait-for-interrupt case.
const WFI_RUNS: u32 = 4;
/// The time between the two cross-hart samples.
const CROSS_SPAN: Duration = Duration::from_secs(1);
/// The bound of a wait for the next timer microsecond, in polls: far above
/// a microsecond's polls.
const EDGE_POLLS: u32 = 100_000;
/// The bound of a direct case's waits for interrupt: far above the wakes
/// within one millisecond.
const WFI_WAKES: u32 = 1_000;
/// The bound of a wait for hart 1's answer, in timer microseconds.
const HART1_ANSWER: u64 = 10_000;
/// The request that ends hart 1's sampling.
const STOP: u32 = u32::MAX;

oer_chips_family_staged_runtime::placed_static! {
    /// The boot hart's latest sample request: written by the boot hart.
    static REQUEST: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
    /// The request hart 1 answered last: written by hart 1.
    static DONE: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
    /// Hart 1's sample of the answered request: timer microseconds and
    /// cycles, each as low and high word; zero time for a failed sample.
    static SAMPLE: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)], in critical_data "hart_exchange";
    /// Hart 1's busy rate: timer microseconds and cycles of its span, each as
    /// low and high word, published before `RATE_READY`.
    static RATE: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)], in critical_data "hart_exchange";
    /// Hart 1 published its rate.
    static RATE_READY: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
}

/// What hart 1 takes at its launch: nothing beyond its handoff.
struct Hart1Parts;

oer_chips_family_staged_runtime::entry!(main, Port);
oer_chips_family_staged_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1_main);

fn main(handoff: oer_chips_family_staged_runtime::Handoff<Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_portable_diagnostics::name!("root-taken"; 32));
    };
    let ((timer, software, watchdogs, reset_causes, hart1), _psram, mut adopted) =
        match oer_chips_family_staged_runtime::adoption::run::<oer_chips_esp32s31_compose::Esp32s31, _, _>(
            handoff,
            root.into_runtime(),
            INTERRUPT_TABLE,
            board::BootstrapPsram,
            oer_chips_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                let reset_causes = oer_chips_esp32s31_compose::reset_causes::read(hardware.reset_cause);
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    reset_causes,
                    hardware.hart1,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_portable_diagnostics::name!("interrupt-table"; 32));
    };
    let timers = oer_chips_family_executor::init::<Esp32s31, 0, _, _, _>(
        oer_chips_family_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let Ok(Some(mut cores)) = Cores::take() else {
        fail(oer_portable_diagnostics::name!("cores-taken"; 32));
    };
    let _hart1 = oer_chips_esp32s31_compose::secondary::launch_hart1(
        &mut cores,
        hart1,
        reset_causes,
        &timers,
        &HART1,
        Hart1Parts,
    )
    .unwrap_or_else(|failure| failure.reset());
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let watchdog = watchdogs.rtc;
    text(b"OER-PROBE mcycle ready\r\n");
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    match executor
        .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err() || Tasks::measure(spawner).is_err() {
                fail(oer_portable_diagnostics::name!("task-start"; 32));
            }
        })
        .unwrap_or_else(|failure| failure.reset())
    {}
}

/// Hart 1's main: its busy rate, then a sample on each of the boot hart's
/// requests until the last, then idle.
fn hart1_main(handoff: SecondaryHandoff<Port, 1>, _parts: Hart1Parts) -> ! {
    let adopted = match handoff.adopt(INTERRUPT_TABLE) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let (micros, cycles) = busy_span().unwrap_or((0, 0));
    store_pair(&RATE, micros, cycles);
    RATE_READY.store(1, Ordering::Release);
    let mut answered = 0;
    loop {
        let request = REQUEST.load(Ordering::Acquire);
        if request == STOP {
            break;
        }
        if request != answered {
            let (micros, cycles) = edge_sample().unwrap_or((0, 0));
            store_pair(&SAMPLE, micros, cycles);
            DONE.store(request, Ordering::Release);
            answered = request;
        }
    }
    match adopted.enable_interrupts() {
        Ok(_router_and_hart) => oer_chips_family_staged_runtime::idle(),
        Err(error) => fail(error.name()),
    }
}

/// Every case in order, then the verdict: every case completed.
async fn measure() {
    // Hart 1 measures its rate while the boot hart waits.
    if EmbassyClock.wait_for(Duration::from_millis(100)).await.is_err() {
        fail(oer_portable_diagnostics::name!("settle-time"; 32));
    }
    let mut complete = true;
    read_costs();
    complete &= busy_rate();
    complete &= hart1_rate();
    let debugger = <Port as oer_chips_family_platform_port::ExecutorPlatform>::debugger_connected(
        oer_chips_family_platform_port::BOOT_HART,
    );
    text(
        Line::new()
            .text("OER-PROBE mcycle debugger hart=0 connected=")
            .number(u64::from(debugger))
            .end(),
    );
    for _ in 0..WFI_RUNS {
        complete &= report_span("direct", direct_wfi().await);
    }
    for _ in 0..WFI_RUNS {
        complete &= report_span("executor", executor_idle().await);
    }
    complete &= cross_hart().await;
    REQUEST.store(STOP, Ordering::Release);
    text(if complete {
        b"OER-RESULT: pass mcycle\r\n"
    } else {
        b"OER-RESULT: fail mcycle\r\n"
    });
}

/// The cycles of `READS` reads of the cycle counter, of the image's clock and
/// of an empty loop, each through `black_box`; called directly, so the stack
/// bound has no indirect call.
fn read_costs() {
    let mcycle = {
        let start = oer_portable_riscv::cycle64();
        for _ in 0..READS {
            core::hint::black_box(oer_portable_riscv::cycle64());
        }
        oer_portable_riscv::cycle64().wrapping_sub(start)
    };
    let now = {
        let start = oer_portable_riscv::cycle64();
        for _ in 0..READS {
            core::hint::black_box(EmbassyClock.now());
        }
        oer_portable_riscv::cycle64().wrapping_sub(start)
    };
    let empty = {
        let start = oer_portable_riscv::cycle64();
        for index in 0..READS {
            core::hint::black_box(index);
        }
        oer_portable_riscv::cycle64().wrapping_sub(start)
    };
    for (name, cycles) in [("mcycle64", mcycle), ("now", now), ("empty", empty)] {
        text(
            Line::new()
                .text("OER-PROBE mcycle read ")
                .text(name)
                .text(" reads=")
                .number(u64::from(READS))
                .text(" cycles=")
                .number(cycles)
                .text(" per-read=")
                .number(cycles / u64::from(READS))
                .end(),
        );
    }
}

/// The boot hart's busy rate.
fn busy_rate() -> bool {
    let span = busy_span();
    report_rate(0, span)
}

/// Hart 1's busy rate, once it published it.
fn hart1_rate() -> bool {
    let start = now_us();
    while RATE_READY.load(Ordering::Acquire) == 0 {
        if now_us().wrapping_sub(start) > HART1_ANSWER {
            text(b"OER-PROBE mcycle rate hart=1 no-answer\r\n");
            return false;
        }
    }
    let (micros, cycles) = load_pair(&RATE);
    report_rate(1, (micros != 0).then_some((micros, cycles)))
}

fn report_rate(hart: u64, span: Option<(u64, u64)>) -> bool {
    let Some((micros, cycles)) = span else {
        text(
            Line::new()
                .text("OER-PROBE mcycle rate hart=")
                .number(hart)
                .text(" no-edge")
                .end(),
        );
        return false;
    };
    text(
        Line::new()
            .text("OER-PROBE mcycle rate hart=")
            .number(hart)
            .text(" busy us=")
            .number(micros)
            .text(" cycles=")
            .number(cycles)
            .text(" per-10ms=")
            .number(per_10ms(cycles, micros).unwrap_or(0))
            .end(),
    );
    true
}

/// One wait-for-interrupt span: its timer microseconds, cycles, retired
/// instructions and the waits it took.
struct Span {
    micros: u64,
    cycles: u64,
    instret: u64,
    waits: u32,
}

/// Wait for interrupt on the boot hart until a timer deadline 1 ms ahead,
/// directly, whatever the executor would do with a debugger attached.
async fn direct_wfi() -> Option<Span> {
    let deadline = EmbassyClock.deadline_after(WFI_SPAN).ok()?;
    let mut wait = WaitUntil::new(deadline);
    let (t0, c0) = edge_sample()?;
    let i0 = oer_portable_riscv::instret64();
    let mut waits = 0_u32;
    // One wait per poll, then back to the executor: its other tasks (the
    // watchdog's feed) run between the waits.
    core::future::poll_fn(|cx| match Pin::new(&mut wait).poll(cx) {
        Poll::Ready(()) => Poll::Ready(()),
        Poll::Pending if waits >= WFI_WAKES => Poll::Ready(()),
        Poll::Pending => {
            oer_portable_riscv::wait_for_interrupt();
            waits = waits.saturating_add(1);
            Poll::Pending
        }
    })
    .await;
    let i1 = oer_portable_riscv::instret64();
    let (t1, c1) = edge_sample()?;
    Some(Span {
        micros: t1.wrapping_sub(t0),
        cycles: c1.wrapping_sub(c0),
        instret: i1.wrapping_sub(i0),
        waits,
    })
}

/// A 1 ms timer wait of the boot hart's task, while the executor idles.
async fn executor_idle() -> Option<Span> {
    let (t0, c0) = edge_sample()?;
    let i0 = oer_portable_riscv::instret64();
    EmbassyClock.wait_for(WFI_SPAN).await.ok()?;
    let i1 = oer_portable_riscv::instret64();
    let (t1, c1) = edge_sample()?;
    Some(Span {
        micros: t1.wrapping_sub(t0),
        cycles: c1.wrapping_sub(c0),
        instret: i1.wrapping_sub(i0),
        waits: 0,
    })
}

fn report_span(kind: &str, span: Option<Span>) -> bool {
    let Some(span) = span else {
        text(
            Line::new()
                .text("OER-PROBE mcycle wfi ")
                .text(kind)
                .text(" hart=0 failed")
                .end(),
        );
        return false;
    };
    text(
        Line::new()
            .text("OER-PROBE mcycle wfi ")
            .text(kind)
            .text(" hart=0 us=")
            .number(span.micros)
            .text(" cycles=")
            .number(span.cycles)
            .text(" per-10ms=")
            .number(per_10ms(span.cycles, span.micros).unwrap_or(0))
            .text(" instret=")
            .number(span.instret)
            .text(" waits=")
            .number(u64::from(span.waits))
            .end(),
    );
    true
}

/// Two samples of both harts' counters, `CROSS_SPAN` apart: each sample's
/// offset of hart 1's counter against hart 0's and the drift between them.
async fn cross_hart() -> bool {
    let Some(rate) = busy_span().and_then(|(micros, cycles)| per_10ms(cycles, micros)) else {
        text(b"OER-PROBE mcycle cross no-rate\r\n");
        return false;
    };
    let Some(first) = cross_sample(1, "a", rate) else {
        return false;
    };
    if EmbassyClock.wait_for(CROSS_SPAN).await.is_err() {
        fail(oer_portable_diagnostics::name!("cross-time"; 32));
    }
    let Some(second) = cross_sample(2, "b", rate) else {
        return false;
    };
    text(
        Line::new()
            .text("OER-PROBE mcycle cross drift cycles=")
            .signed(second.1.saturating_sub(first.1))
            .text(" over-us=")
            .number(second.0.wrapping_sub(first.0))
            .text(" rate-per-10ms=")
            .number(rate)
            .end(),
    );
    true
}

/// Ask hart 1 for a sample and take one on this hart at once; report both
/// and hart 1's offset. The boot hart's time and the offset.
fn cross_sample(request: u32, name: &str, rate: u64) -> Option<(u64, i64)> {
    REQUEST.store(request, Ordering::Release);
    let hart0 = edge_sample();
    let start = now_us();
    while DONE.load(Ordering::Acquire) != request {
        if now_us().wrapping_sub(start) > HART1_ANSWER {
            text(b"OER-PROBE mcycle cross no-answer\r\n");
            return None;
        }
    }
    let hart1 = load_pair(&SAMPLE);
    let (Some(hart0), true) = (hart0, hart1.0 != 0) else {
        text(b"OER-PROBE mcycle cross no-edge\r\n");
        return None;
    };
    let lead = offset(hart0, hart1, rate)?;
    text(
        Line::new()
            .text("OER-PROBE mcycle cross sample=")
            .text(name)
            .text(" t0=")
            .number(hart0.0)
            .text(" c0=")
            .number(hart0.1)
            .text(" t1=")
            .number(hart1.0)
            .text(" c1=")
            .number(hart1.1)
            .text(" offset=")
            .signed(lead)
            .end(),
    );
    Some((hart0.0, lead))
}

/// A busy span of `RATE_SPAN` timer microseconds between two edges: its
/// microseconds and cycles.
fn busy_span() -> Option<(u64, u64)> {
    let (t0, c0) = edge_sample()?;
    let end = t0.checked_add(RATE_SPAN)?;
    while now_us() < end {}
    let (t1, c1) = edge_sample()?;
    Some((t1.wrapping_sub(t0), c1.wrapping_sub(c0)))
}

/// The first timer microsecond after the current one and the cycle counter
/// read right after it was seen; `None` when the timer does not advance.
fn edge_sample() -> Option<(u64, u64)> {
    let start = now_us();
    for _ in 0..EDGE_POLLS {
        let now = now_us();
        if now != start {
            return Some((now, oer_portable_riscv::cycle64()));
        }
    }
    None
}

/// The image's clock in microseconds.
fn now_us() -> u64 {
    EmbassyClock.now().as_micros()
}

fn store_pair(words: &[AtomicU32; 4], micros: u64, cycles: u64) {
    let [t_low, t_high, c_low, c_high] = words;
    t_low.store(micros as u32, Ordering::Relaxed);
    t_high.store((micros >> 32) as u32, Ordering::Relaxed);
    c_low.store(cycles as u32, Ordering::Relaxed);
    c_high.store((cycles >> 32) as u32, Ordering::Relaxed);
}

fn load_pair(words: &[AtomicU32; 4]) -> (u64, u64) {
    let [t_low, t_high, c_low, c_high] = words;
    let join = |low: &AtomicU32, high: &AtomicU32| {
        (u64::from(high.load(Ordering::Relaxed)) << 32) | u64::from(low.load(Ordering::Relaxed))
    };
    (join(t_low, t_high), join(c_low, c_high))
}

/// Feed the RTC watchdog for good.
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
