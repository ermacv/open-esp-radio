//! Turns a silent stall of either executor into a post-mortem and a reset.
//!
//! The core 0 protocol executor and the core 1 network executor each advance
//! a heartbeat. A periodic SYSTIMER alarm 0 interrupt at the highest priority
//! checks both every 250 ms. Once one of them has made no progress for 3 s,
//! it samples at each of 16 further checks the instruction the stalled hart
//! was interrupted at: its own `mepc` for core 0, and for core 1 the context
//! core 1's sample handler reports (FROM_CPU line 2, shared with the profiler;
//! see `software_interrupt`). The pending interrupt
//! sources are not needed: the samples tell a busy loop from a wait. It then
//! records the hang, with both harts' interrupted `mepc`, `ra`, `sp`,
//! `mcause` and `mstatus`, in the post-mortem record, prints one
//! `hil-postmortem:` line and resets the chip, so the next boot reports it.
//!
//! A task can also hang while both executors run: it awaits forever while
//! work waits for it. Such a task owns a slot in [`LIVENESS`], armed while
//! its work waits (see `oer_hil_agent::liveness`); a slot past its
//! deadline is sampled and recorded like a stalled executor, with core 0's
//! samples and the slot named instead of an executor.
//!
//! The core 1 executor's timers are driven by core 0's time driver, so a
//! core 0 executor that stops at the interrupt level of that driver or above
//! stalls both heartbeats; the samples then come from core 0.
//!
//! A hart that spins with its interrupts masked never takes the sampling
//! interrupt: its state is reported as not responding, which is itself
//! evidence. If core 0 itself is masked, the watchdog never runs and TIMG1's
//! deadline watchdog and the RTC watchdog remain.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use critical_section::Mutex;
use esp_hal::{
    Blocking,
    time::Duration,
    timer::{PeriodicTimer, systimer::Alarm},
};
use oer_hil_agent::liveness::TaskLiveness;
use oer_hil_protocol::base::{HangFault, HartState, TaskSlot};

/// Tasks with work waiting for them.
pub(crate) static LIVENESS: TaskLiveness = TaskLiveness::new();

/// Milliseconds since boot, the clock of [`LIVENESS`].
pub(crate) fn uptime_ms() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis() as u32
}

/// Work for `slot`'s task arrived.
pub(crate) fn arm(slot: TaskSlot) {
    LIVENESS.arm(slot, uptime_ms());
}

/// `slot`'s task took work; `more` tells whether work is still waiting.
pub(crate) fn took_work(slot: TaskSlot, more: bool) {
    if more {
        LIVENESS.progress(slot, uptime_ms());
    } else {
        LIVENESS.disarm(slot);
    }
}

/// Set by a diagnostic console hang: the console stops taking commands.
static CONSOLE_STALLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Stop the console's command consumer after its current command.
pub(crate) fn inject_console_stall() {
    CONSOLE_STALLED.store(true, Ordering::Release);
}

/// Park the console's command consumer forever once a diagnostic console
/// hang was injected; its executor keeps running.
pub(crate) async fn console_stall_point() {
    if CONSOLE_STALLED.load(Ordering::Acquire) {
        core::future::pending::<()>().await;
    }
}

/// Heartbeats of the core 0 protocol and core 1 network executors.
static HEARTBEATS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static LAST_SEEN: [AtomicU32; 2] = [const { AtomicU32::new(u32::MAX) }; 2];
static STALE_CHECKS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static TIMER: Mutex<RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

/// Core 1's answer to the latest sampling interrupt: responses so far, then
/// `mepc`, `ra`, `sp`, `mcause`, `mstatus`.
static CORE1_RESPONSES: AtomicU32 = AtomicU32::new(0);
static CORE1_CONTEXT: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];

const CHECK_PERIOD_MILLIS: u64 = 250;
const STALE_CHECK_LIMIT: u32 = 12;
const SAMPLE_COUNT: usize = 16;
static SAMPLES: [AtomicU32; SAMPLE_COUNT] = [const { AtomicU32::new(0) }; SAMPLE_COUNT];
static SAMPLED: AtomicU32 = AtomicU32::new(0);
/// Core 1's responses when the current stall was first sampled.
static RESPONSES_AT_STALL: AtomicU32 = AtomicU32::new(0);

/// An executor whose progress the watchdog checks.
#[derive(Clone, Copy)]
pub(crate) enum Executor {
    /// The core 0 protocol executor.
    Protocol = 0,
    /// The core 1 network executor.
    Network = 1,
}

/// Executors a diagnostic hang stalls, one bit per [`Executor`].
static INJECTED: AtomicU32 = AtomicU32::new(0);

/// Stall `executor` at its next heartbeat, with interrupts enabled.
pub(crate) fn inject(executor: Executor) {
    INJECTED.fetch_or(1 << executor as u32, Ordering::Release);
}

fn stall_if_injected(executor: Executor) {
    if INJECTED.load(Ordering::Acquire) & (1 << executor as u32) != 0 {
        loop {
            #[allow(
                clippy::disallowed_methods,
                reason = "a diagnostic hang spins on purpose until the watchdog resets the chip"
            )]
            core::hint::spin_loop();
        }
    }
}

/// Advance `executor`'s heartbeat.
pub(crate) fn heartbeat(executor: Executor) {
    HEARTBEATS[executor as usize].fetch_add(1, Ordering::Relaxed);
}

/// Start the watchdog on SYSTIMER alarm 0; `token` is its source's, whose
/// table entry names [`check`] on this core.
pub(super) fn start(alarm: Alarm<'static>, token: crate::HangWatchdogCheck) {
    let mut periodic = PeriodicTimer::new(alarm);
    if let Err(error) = oer_espressif_interrupt_table_esp_hal::enable(&token) {
        panic!("hang watchdog alarm: {error:?}");
    }
    periodic
        .start(Duration::from_millis(CHECK_PERIOD_MILLIS))
        .expect("the watchdog period fits the timer");
    periodic.listen();
    critical_section::with(|cs| TIMER.borrow_ref_mut(cs).replace(periodic));
}

fn raise_core1_sample() {
    crate::software_interrupt::request_core1_sample(
        oer_hil_agent::sample_request::Requester::HangWatchdog,
    );
}

/// This hart's interrupted context, as the running handler sees it.
fn interrupted() -> [u32; 5] {
    let (mepc, mcause, mstatus): (usize, usize, usize);
    // SAFETY: reading machine CSRs has no side effect.
    #[allow(unsafe_code, reason = "reading CSRs requires CSR instructions")]
    unsafe {
        core::arch::asm!("csrr {0}, mepc", out(reg) mepc);
        core::arch::asm!("csrr {0}, mcause", out(reg) mcause);
        core::arch::asm!("csrr {0}, mstatus", out(reg) mstatus);
    }
    let context = esp_hal::interrupt::interrupted_context().unwrap_or_default();
    let sp = oer_espressif_staged_runtime::stacks::interrupted_stack_pointer().unwrap_or(0);
    [
        mepc as u32,
        context.ra as u32,
        sp as u32,
        mcause as u32,
        mstatus as u32,
    ]
}

fn hart(responded: bool, [mepc, ra, sp, mcause, mstatus]: [u32; 5]) -> HartState {
    HartState {
        responded,
        mepc,
        ra,
        sp,
        mcause,
        mstatus,
    }
}

/// Record core 1's interrupted context for the watchdog's request; called
/// by core 1's sample handler (`software_interrupt::sample_core1`).
#[inline(always)]
pub(crate) fn record_core1_context() {
    for (slot, value) in CORE1_CONTEXT.iter().zip(interrupted()) {
        slot.store(value, Ordering::Relaxed);
    }
    CORE1_RESPONSES.fetch_add(1, Ordering::Release);
}

/// An interrupt-table handler of the image.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub(crate) fn check() {
    critical_section::with(|cs| {
        if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
            timer.clear_interrupt();
        }
    });
    let mut stalled = 0_u8;
    for executor in 0..2 {
        let beat = HEARTBEATS[executor].load(Ordering::Relaxed);
        // An executor is watched from its first heartbeat: the protocol
        // executor's starts only once the host initializes the image.
        if beat == 0 {
            continue;
        }
        if LAST_SEEN[executor].swap(beat, Ordering::Relaxed) == beat {
            if STALE_CHECKS[executor].fetch_add(1, Ordering::Relaxed) + 1 >= STALE_CHECK_LIMIT {
                stalled |= 1 << executor;
            }
        } else {
            STALE_CHECKS[executor].store(0, Ordering::Relaxed);
        }
    }
    // A task is watched only while both executors run: an executor stall
    // explains any task on it.
    let task = if stalled == 0 {
        LIVENESS.overdue(uptime_ms())
    } else {
        None
    };
    let responses = CORE1_RESPONSES.load(Ordering::Acquire);
    // Core 1 reports its context from the first missed heartbeat on, so the
    // first sample of a stall is already current.
    let lagging = STALE_CHECKS
        .iter()
        .any(|stale| stale.load(Ordering::Relaxed) != 0);
    if stalled == 0 && task.is_none() {
        SAMPLED.store(0, Ordering::Relaxed);
        if lagging {
            raise_core1_sample();
        }
        return;
    }
    // Sample the stalled hart: core 1 answers the previous check's request.
    let index = SAMPLED.fetch_add(1, Ordering::Relaxed) as usize;
    if index == 0 {
        RESPONSES_AT_STALL.store(responses, Ordering::Relaxed);
    }
    // A stalled task is sampled on core 0, where the console and session
    // tasks run.
    let sample = if stalled & 1 != 0 || stalled == 0 {
        interrupted()[0]
    } else {
        CORE1_CONTEXT[0].load(Ordering::Relaxed)
    };
    if let Some(slot) = SAMPLES.get(index) {
        slot.store(sample, Ordering::Relaxed);
    }
    if index + 1 < SAMPLE_COUNT {
        raise_core1_sample();
        return;
    }
    // Core 1 answered most requests during this stall: its interrupts are on.
    let core1_responded = responses.wrapping_sub(RESPONSES_AT_STALL.load(Ordering::Relaxed))
        >= SAMPLE_COUNT as u32 / 2;
    let hang = HangFault {
        detected_uptime_ms: esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis() as u32,
        stalled_executors: stalled,
        harts: [
            hart(true, interrupted()),
            hart(
                core1_responded,
                core::array::from_fn(|index| CORE1_CONTEXT[index].load(Ordering::Relaxed)),
            ),
        ],
        samples: core::array::from_fn(|index| SAMPLES[index].load(Ordering::Relaxed)),
        stalled_task: task,
    };
    // The trace keeps what happened before the hang.
    oer_trace::freeze(<oer_hil_trace::Hang as oer_trace::Event>::KIND, 0);
    crate::system::postmortem::record_hang(&hang);
    esp_hal::system::software_reset()
}

/// The core 0 protocol executor's heartbeat.
#[embassy_executor::task]
pub(super) async fn protocol_heartbeat_task() {
    loop {
        stall_if_injected(Executor::Protocol);
        heartbeat(Executor::Protocol);
        embassy_time::Timer::after_millis(100).await;
    }
}

/// The core 1 network executor's heartbeat.
#[embassy_executor::task]
pub(super) async fn network_heartbeat_task() {
    loop {
        stall_if_injected(Executor::Network);
        heartbeat(Executor::Network);
        embassy_time::Timer::after_millis(100).await;
    }
}
