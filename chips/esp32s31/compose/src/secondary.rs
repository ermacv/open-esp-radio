//! Hart 1's launch during boot, before the executors start: a boot deadline
//! of its own on the time driver's clock, which [`launch_hart1`] requires
//! installed (its `TimeDriverReady` authority), the launch waits below and
//! the staged runtime's launch ([`oer_espressif_staged_runtime::launch`]),
//! which hands hart 1 its interrupt grant and the image's parts through the
//! image's mailbox (`secondary_entry!`). The parts may carry a timer grant
//! for hart 1's executor (`oer_espressif_executor::grant`), which the time
//! driver issues only once installed, and hardware such as `AesParts` that
//! the image places on hart 1.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::{
    cores::{Cores, LaunchPermit, StartError},
    peripherals::reset_cause::ResetCauses,
    platform::Port,
};
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::{LaunchFailure, LaunchWaits, Mailbox, Running};

/// The boot deadline of a secondary hart's launch, from the release to its
/// ready report: well below the RTC watchdog the runtime armed.
#[cfg(target_arch = "riscv32")]
pub const LAUNCH_BUDGET: oer_espressif_boot::wait::BootBudget =
    match oer_espressif_boot::wait::BootBudget::below(
        oer_time::Duration::from_millis(100),
        oer_time::Duration::from_millis(
            oer_esp32s31_hal::peripherals::watchdog::RTC_WATCHDOG_BUDGET_MS,
        ),
    ) {
        Some(budget) => budget,
        None => panic!("the launch budget lies below the RTC watchdog"),
    };

/// From the release to the entry's report (`secondary-entered`).
///
/// SOURCE: HW E1 of 2026-10-31 on S31 rev0 (oer-notes
/// research/hw-hart1-entry-e1.md): 5952 CPU cycles from the release to the
/// probe entry's last marker through the ROM `main` path, which includes the
/// whole entry and a CSR snapshot; this bound is over a thousand times longer.
#[cfg(target_arch = "riscv32")]
pub const LAUNCH_ENTERED_WAIT: oer_espressif_boot::wait::BootWait = const {
    match oer_espressif_boot::wait::BootWait::new(
        "secondary-entered",
        oer_time::Duration::from_millis(10),
    ) {
        Ok(wait) => wait,
        Err(_) => panic!("a boot wait has a nonzero bound"),
    }
};

/// From the acknowledgement to the hart's ready report (`secondary-ready`):
/// the PMA program, the interrupt stack's paint (at most the 32 KiB contract
/// maximum of SRAM words) and the exception path's installation.
///
/// SOURCE: the same E1 measurement; the paint adds one store per word.
#[cfg(target_arch = "riscv32")]
pub const LAUNCH_READY_WAIT: oer_espressif_boot::wait::BootWait = const {
    match oer_espressif_boot::wait::BootWait::new(
        "secondary-ready",
        oer_time::Duration::from_millis(50),
    ) {
        Ok(wait) => wait,
        Err(_) => panic!("a boot wait has a nonzero bound"),
    }
};

#[cfg(target_arch = "riscv32")]
const _: () = {
    if !LAUNCH_BUDGET.admits(LAUNCH_ENTERED_WAIT) || !LAUNCH_BUDGET.admits(LAUNCH_READY_WAIT) {
        panic!("each launch wait fits the launch budget");
    }
};

/// Why hart 1 did not start: its launch failed after its deadline started.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hart1Failure(pub LaunchFailure<StartError>);

#[cfg(target_arch = "riscv32")]
impl Hart1Failure {
    /// Record the failure and reset the chip: a released hart is not
    /// confirmed held again.
    pub fn reset(self) -> ! {
        self.0.reset::<Port>()
    }
}

/// Launch hart 1 with the root's one-shot `permit`, under a deadline of its
/// own on the time driver's clock, which `_timers` proves installed (the
/// counter latch checked and published), handing it its interrupt grant and
/// `parts` through `mailbox`, and return the running hart.
///
/// # Errors
///
/// [`Hart1Failure`]; the image resets ([`Hart1Failure::reset`]).
#[cfg(target_arch = "riscv32")]
pub fn launch_hart1<T: Send>(
    cores: &mut Cores,
    permit: LaunchPermit,
    causes: ResetCauses,
    _timers: &oer_espressif_executor::TimeDriverReady<crate::Esp32s31>,
    mailbox: &'static Mailbox<oer_esp32s31_hal::platform::Hart1Grant, T>,
    parts: T,
) -> Result<Running<Port, 1>, Hart1Failure> {
    let clock = oer_embassy_time::EmbassyClock;
    let deadline = oer_espressif_boot::wait::BootDeadline::start(&clock, LAUNCH_BUDGET);
    oer_espressif_staged_runtime::launch::<Port, _, 1, T>(
        cores,
        permit,
        causes,
        &deadline,
        LaunchWaits {
            entered: LAUNCH_ENTERED_WAIT,
            ready: LAUNCH_READY_WAIT,
        },
        mailbox,
        parts,
    )
    .map_err(Hart1Failure)
}
