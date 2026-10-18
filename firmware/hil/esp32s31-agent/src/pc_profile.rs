//! The ESP32-S31 sampling side of the program-counter profile.
//!
//! SYSTIMER alarm 1 interrupts core 0 at the highest priority every armed
//! period. Its handler records the instruction core 0 was interrupted at
//! (`mepc`) and the interrupted code's return address, then requests core
//! 1's sample (FROM_CPU line 2, shared with the hang watchdog; see
//! `software_interrupt`), whose handler bound on core 1 records core 1's pair
//! the same way. Samples cover interrupt handlers of lower priority,
//! executor tasks and the idle wait alike, so the profile divides each hart's
//! whole time, not only task polls.
//!
//! The chip-neutral [`Profiler`] keeps the samples and the window state. The
//! host arms and drains it through the console; the UDP RX workload opens and
//! closes the measured window. The return address names the caller only
//! while the sampled function has not yet made a call of its own; it is
//! exact for leaf functions such as copies and checksums.

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_time::Instant;
use esp_hal::{
    Blocking,
    time::Duration,
    timer::{PeriodicTimer, systimer::Alarm},
};
use oer_firmware_hil_agent::profile::{PageRefusal, ProfileTimer, Profiler};
use oer_tool_data_hil_protocol::{base::RejectReason, telemetry::ProfileControl};

/// Samples retained per hart: a 16-second window at a 2-ms period.
const CAPACITY: usize = 8192;
/// Shortest accepted period. Each sample costs two high-priority interrupts;
/// a shorter period would measure the profiler instead of the workload.
const MINIMUM_PERIOD_MICROS: u32 = 200;
/// Longest accepted period, within one SYSTIMER alarm period.
const MAXIMUM_PERIOD_MICROS: u32 = 1_000_000;

oer_base_memory::zeroed_static! {
    /// Placement: the sample buffers are placed in the PSRAM tier to keep internal SRAM for the measured path.
    static PROFILER: Profiler<CAPACITY> =
        zeroed in ".psram.bss.open_radio_pc_profile";
}
static TIMER: Mutex<RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

/// SYSTIMER alarm 1 as the profile's sampling timer.
struct SystimerProfileTimer;

impl ProfileTimer for SystimerProfileTimer {
    fn start(&mut self, period_us: u32) {
        critical_section::with(|cs| {
            if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
                timer
                    .start(Duration::from_micros(u64::from(period_us)))
                    .expect("an accepted profile period fits the timer");
                timer.listen();
            }
        });
    }

    fn stop(&mut self) {
        critical_section::with(|cs| {
            if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
                timer.unlisten();
                let _ = timer.cancel();
                timer.clear_interrupt();
            }
        });
    }
}

/// Own the sampling alarm on this core (core 0); it stays idle until armed.
/// `token` is its source's, whose table entry names [`sample_core0`].
pub(crate) fn init(alarm: Alarm<'static>, token: crate::ProfileSample) {
    let periodic = PeriodicTimer::new(alarm);
    if let Err(error) = oer_exec_esp_hal_irq::enable(&token) {
        panic!("profile alarm: {error:?}");
    }
    critical_section::with(|cs| TIMER.borrow_ref_mut(cs).replace(periodic));
}

/// Serve one host profile command.
pub(crate) fn control(
    control: ProfileControl,
) -> Result<oer_tool_data_hil_protocol::telemetry::ProfileState, RejectReason> {
    match control {
        ProfileControl::Arm { harts, period_us } => {
            if !(MINIMUM_PERIOD_MICROS..=MAXIMUM_PERIOD_MICROS).contains(&period_us) {
                return Err(RejectReason::InvalidConfiguration);
            }
            SystimerProfileTimer.stop();
            PROFILER.arm(harts, period_us);
            SystimerProfileTimer.start(period_us);
        }
        ProfileControl::Disarm => {
            PROFILER.disarm();
            SystimerProfileTimer.stop();
        }
        ProfileControl::Status => {}
    }
    Ok(oer_tool_data_hil_protocol::telemetry::ProfileState(PROFILER.status()))
}

/// Serve one page of a closed window's samples.
pub(crate) fn samples(
    hart: u8,
    first: u32,
) -> Result<oer_tool_data_hil_protocol::telemetry::ProfileSamples, RejectReason> {
    match PROFILER.page(usize::from(hart), first) {
        Ok(page) => Ok(oer_tool_data_hil_protocol::telemetry::ProfileSamples(page)),
        Err(PageRefusal::WindowOpen | PageRefusal::Hart) => Err(RejectReason::InvalidState),
    }
}

/// The workload's measured window begins.
pub(crate) fn window_begin() {
    PROFILER.window_begin(now_micros());
}

/// The workload's measured window ends.
pub(crate) fn window_end() {
    PROFILER.window_end(now_micros());
}

fn now_micros() -> u32 {
    // The profiler measures the window with wrapping 32-bit arithmetic.
    Instant::now().as_micros() as u32
}

fn record(hart: usize) {
    let mepc: usize;
    // SAFETY: reading a machine CSR has no side effect.
    #[allow(unsafe_code, reason = "reading a CSR requires a CSR instruction")]
    unsafe {
        core::arch::asm!("csrr {0}, mepc", out(reg) mepc);
    }
    let ra = esp_hal::interrupt::interrupted_context()
        .map(|context| context.ra)
        .unwrap_or(0);
    PROFILER.record(hart, mepc as u32, ra as u32);
}

/// An interrupt-table handler of the image.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub(crate) fn sample_core0() {
    critical_section::with(|cs| {
        if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
            timer.clear_interrupt();
        }
    });
    record(0);
    if PROFILER.samples_wanted(1) {
        crate::software_interrupt::request_core1_sample(
            oer_firmware_hil_agent::sample_request::Requester::Profiler,
        );
    }
}

/// Record core 1's sample for the profiler's request; called by core 1's
/// sample handler (`software_interrupt::sample_core1`).
#[inline(always)]
pub(crate) fn record_core1() {
    record(1);
}
