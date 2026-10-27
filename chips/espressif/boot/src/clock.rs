//! The CPU clock profile of an image, brought up once by the bootstrap.
//!
//! A profile names the CPU root clock, the PLL that root needs (if any), the
//! divider of every branch the root feeds and the rates they give. Bringing
//! it up:
//!
//! 1. reads the live root. When the CPU already runs from the profile's root
//!    the PLL is never recalibrated, since that would disturb the clock the
//!    CPU runs on; its configuration must read back as the profile's, or the
//!    boot fails. Otherwise the PLL is configured, powered (the immediate
//!    ties and gate, then the PMU's active state), calibrated (a
//!    boot wait for its calibration's end, then the fixed settle its source
//!    states) and its calibration stopped;
//! 2. stages the root selection and every divider, then applies them all
//!    with one update (started once no update runs) and waits for it to
//!    complete, so no branch
//!    runs over its limit between two partial updates;
//! 3. reads the root and the dividers back.
//!
//! When the root and the dividers already read as the profile's, nothing is
//! written.
//!
//! SOURCE: ESP-IDF 4d80c1b2
//! `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:175-195,422-478,599-605`
//! (CPLL power, dividers, calibration and the bus update);
//! `components/esp_hw_support/port/esp32s31/rtc_clk.c:133-148,404-432`
//! (calibration start, completion, 10 µs settle, stop; prepare the PLL
//! when the CPU source changes, then switch the CPU configuration).

use oer_time::Clock;

use crate::{
    Access, BootFailure, Busy,
    wait::{BootDeadline, BootWait},
};

/// The CPU root selection and the branch dividers, staged without effect
/// until [`ClockUpdate`] applies them.
pub trait CpuClockRegisters {
    /// The chip's CPU root clocks.
    type Root: Copy + Eq;
    /// The chip's divider of every branch the root feeds.
    type Dividers: Copy + Eq;
    /// The live root, or `None` when its selector holds a code the chip
    /// does not name.
    fn root(&self, access: Access<'_>) -> Option<Self::Root>;
    /// The dividers as they read now.
    fn dividers(&self, access: Access<'_>) -> Self::Dividers;
    /// Write the root selection and every divider, applied only by the next
    /// update.
    fn stage(&mut self, access: Access<'_>, root: Self::Root, dividers: Self::Dividers);
}

/// The update that applies a staged clock configuration: a command the
/// hardware clears when it completes.
pub trait ClockUpdate {
    /// Start the update, or [`Busy`] while one runs.
    fn start_update(&mut self, access: Access<'_>) -> Result<(), Busy>;
    /// Whether the update is still in progress.
    fn update_pending(&self, access: Access<'_>) -> bool;
}

/// A PLL's configuration and calibration.
pub trait PllRegisters {
    /// The PLL's configuration (its divider pair).
    type Target: Copy + Eq;
    /// The configuration as it reads now.
    fn target(&self, access: Access<'_>) -> Self::Target;
    /// Write the configuration.
    fn set_target(&mut self, access: Access<'_>, target: Self::Target);
    /// Allow the calibration to run.
    fn start_calibration(&mut self, access: Access<'_>);
    /// Whether the calibration has ended.
    fn calibration_done(&self, access: Access<'_>) -> bool;
    /// Stop the calibration.
    fn stop_calibration(&mut self, access: Access<'_>);
}

/// A PLL's power and clock gates, which its calibration needs: the PMU's
/// immediate ties and the PLL's clock gate.
pub trait PllPower {
    /// Power the PLL and open its gates, in the chip's order.
    fn power_up(&mut self, access: Access<'_>);
}

/// The PLLs' power in the PMU's active state, which the PMU applies again on
/// every state change: a PLL the boot powers only by the immediate ties
/// would stop at the first one. The words belong to another block than the
/// clock's, so the sequences take them as a separate owner.
pub trait ActivePllPower {
    /// Keep the CPU root's PLL powered in the active state.
    fn keep_cpu_pll(&mut self, access: Access<'_>);
    /// Keep the external memory's PLL powered in the active state.
    fn keep_memory_pll(&mut self, access: Access<'_>);
}

/// The rates of a profile's branches, in hertz: the snapshot the boot hands
/// to stage two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockRates {
    pub cpu_hz: u32,
    pub mem_hz: u32,
    pub sys_hz: u32,
    pub apb_hz: u32,
}

/// One CPU clock profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuClockProfile<Root, Dividers, Target> {
    /// The root the CPU runs from.
    pub root: Root,
    /// The configuration of the PLL the root is, if it is one.
    pub pll: Option<Target>,
    /// The divider of every branch.
    pub dividers: Dividers,
    /// The rates the root and the dividers give.
    pub rates: ClockRates,
}

/// The named waits of a clock bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockWaits {
    /// The PLL calibration's end.
    pub calibration: BootWait,
    /// The fixed settle after the calibration ended, before it is stopped.
    pub calibration_settle: BootWait,
    /// The clock update's completion.
    pub update: BootWait,
}

/// Why the CPU clock did not reach its profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockFailure {
    /// The root selector holds a code the chip does not name.
    UnknownRoot,
    /// The CPU runs from the profile's PLL, but the PLL reads another
    /// configuration; recalibrating it under the CPU is refused.
    LivePllMismatch,
    /// The root or the dividers read back otherwise after the update.
    Readback,
}

impl ClockFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnknownRoot => "clock-root-unknown",
            Self::LivePllMismatch => "clock-live-pll",
            Self::Readback => "clock-readback",
        }
    }
}

/// Bring the CPU clock to `profile` under `deadline` and return its rates.
pub fn bring_up<R, A, C>(
    registers: &mut R,
    active: &mut A,
    profile: CpuClockProfile<R::Root, R::Dividers, R::Target>,
    waits: ClockWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<ClockRates, BootFailure>
where
    R: CpuClockRegisters + ClockUpdate + PllRegisters + PllPower,
    A: ActivePllPower,
    C: Clock + ?Sized,
{
    let live = registers
        .root(Access::new())
        .ok_or(ClockFailure::UnknownRoot)?;
    if let Some(target) = profile.pll {
        if live == profile.root {
            if registers.target(Access::new()) != target {
                return Err(ClockFailure::LivePllMismatch.into());
            }
        } else {
            calibrate(registers, active, target, waits, deadline)?;
        }
    }
    if live != profile.root || registers.dividers(Access::new()) != profile.dividers {
        registers.stage(Access::new(), profile.root, profile.dividers);
        deadline.poll(waits.update, || registers.start_update(Access::new()).ok())?;
        deadline.poll(waits.update, || {
            (!registers.update_pending(Access::new())).then_some(())
        })?;
    }
    if registers.root(Access::new()) != Some(profile.root)
        || registers.dividers(Access::new()) != profile.dividers
    {
        return Err(ClockFailure::Readback.into());
    }
    Ok(profile.rates)
}

fn calibrate<R, A, C>(
    registers: &mut R,
    active: &mut A,
    target: R::Target,
    waits: ClockWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure>
where
    R: PllRegisters + PllPower,
    A: ActivePllPower,
    C: Clock + ?Sized,
{
    registers.set_target(Access::new(), target);
    registers.power_up(Access::new());
    active.keep_cpu_pll(Access::new());
    registers.start_calibration(Access::new());
    deadline.poll(waits.calibration, || {
        registers.calibration_done(Access::new()).then_some(())
    })?;
    deadline.hold(waits.calibration_settle)?;
    registers.stop_calibration(Access::new());
    Ok(())
}
