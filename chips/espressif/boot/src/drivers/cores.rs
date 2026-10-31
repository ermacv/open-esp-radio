//! Boot containment of the profile's secondary harts, a secondary hart's
//! release into its entry and the retirement of its boot word.
//! SOURCE: esp-hal 1f646c5 `soc/esp32s31/cpu_control.rs:6-96`;
//! ESP-IDF 4d80c1b2 `system_internal.c:61-100`.
use crate::Access;

pub trait Registers {
    type Secondary: Copy + 'static;
    const SECONDARIES: &'static [Self::Secondary];
    fn request_stall(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn hold(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn clear_boot_address(&mut self, access: Access<'_>, hart: Self::Secondary);
}

/// Stall, gate and hold each secondary, then clear its ROM entry.
pub fn contain<R: Registers>(registers: &mut R) {
    for &hart in R::SECONDARIES {
        registers.request_stall(Access::new(), hart);
        registers.hold(Access::new(), hart);
        registers.clear_boot_address(Access::new(), hart);
    }
}

/// Completed stall-status reads per request. This counts reads, not elapsed
/// time: ESP-IDF stalls without waiting and waits unboundedly for a run. No
/// source states hardware latency, and a read that never completes is not
/// bounded by this budget.
pub const STALL_POLLS: u32 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unconfirmed;

/// How a secondary stands, sampled held first and stalled only if not held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreState {
    Held,
    Stalled,
    Released,
}

/// What confirmed a peer stopped; this grants no peripheral takeover.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stopped {
    Held,
    Stalled,
}

/// Core-control atoms. The HAL lends these only after its ownership or fatal
/// election succeeded; this driver owns no election, retained state or reset.
pub trait RuntimeRegisters: Registers {
    type Hart: Copy + Eq + 'static;
    type Stops: Default;
    const HARTS: &'static [Self::Hart];
    fn hart(secondary: Self::Secondary) -> Self::Hart;
    fn save_stop(stops: &mut Self::Stops, hart: Self::Hart, stopped: Stopped);
    fn fence(&self, access: Access<'_>);
    fn held(&self, access: Access<'_>, hart: Self::Hart) -> bool;
    fn is_stalled(&self, access: Access<'_>, hart: Self::Hart) -> bool;
    fn request_peer_stall(&mut self, access: Access<'_>, hart: Self::Hart);
    fn request_run(&mut self, access: Access<'_>, hart: Self::Hart);
    fn reset_core(&mut self, access: Access<'_>, hart: Self::Hart);
}

/// Wait for exactly the requested status, with fences before sampling and
/// after confirmation. A failure has no final fence, as in the original order.
fn confirm<R: RuntimeRegisters>(
    registers: &R,
    hart: R::Hart,
    stalled: bool,
    polls: u32,
) -> Result<(), Unconfirmed> {
    registers.fence(Access::new());
    for _ in 0..polls {
        if registers.is_stalled(Access::new(), hart) == stalled {
            registers.fence(Access::new());
            return Ok(());
        }
    }
    Err(Unconfirmed)
}

pub fn stall<R: RuntimeRegisters>(
    registers: &mut R,
    hart: R::Hart,
    polls: u32,
) -> Result<(), Unconfirmed> {
    registers.request_peer_stall(Access::new(), hart);
    confirm(registers, hart, true, polls)
}

pub fn run<R: RuntimeRegisters>(
    registers: &mut R,
    hart: R::Hart,
    polls: u32,
) -> Result<(), Unconfirmed> {
    registers.request_run(Access::new(), hart);
    confirm(registers, hart, false, polls)
}

pub fn state<R: RuntimeRegisters>(registers: &R, hart: R::Secondary) -> CoreState {
    let hart = R::hart(hart);
    if registers.held(Access::new(), hart) {
        CoreState::Held
    } else if registers.is_stalled(Access::new(), hart) {
        CoreState::Stalled
    } else {
        CoreState::Released
    }
}

/// Request a secondary's stall, then gate its clocks and hold its reset.
pub fn park<R: Registers>(registers: &mut R, hart: R::Secondary) {
    registers.request_stall(Access::new(), hart);
    registers.hold(Access::new(), hart);
}

/// Confirm every peer in geometry order, without sampling or stalling self.
pub fn stop_other<R: RuntimeRegisters>(
    registers: &mut R,
    this: R::Hart,
    polls: u32,
) -> Result<R::Stops, Unconfirmed> {
    let mut stopped = R::Stops::default();
    for &hart in R::HARTS {
        if hart == this {
            continue;
        }
        let confirmation = if registers.held(Access::new(), hart) {
            Stopped::Held
        } else {
            stall(registers, hart, polls)?;
            Stopped::Stalled
        };
        R::save_stop(&mut stopped, hart, confirmation);
    }
    Ok(stopped)
}

/// Clear every secondary ROM entry before any peer reset pulse. The owner
/// issues the final fence and system reset; the driver never resets the chip.
/// SOURCE: ESP-IDF 4d80c1b2 `system_internal.c:61-100`.
pub fn prepare_reset<R: RuntimeRegisters>(registers: &mut R, this: R::Hart) {
    clear_boot_addresses(registers);
    for &hart in R::HARTS {
        if hart != this {
            registers.reset_core(Access::new(), hart);
            registers.request_peer_stall(Access::new(), hart);
        }
    }
}

/// The atoms of a secondary hart's release: its clocks, its global reset and
/// its ROM boot word.
pub trait LaunchRegisters: RuntimeRegisters {
    fn enable_cpu_clock(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn enable_clic_clock(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn release_reset(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn set_boot_address(&mut self, access: Access<'_>, hart: Self::Secondary, entry: u32);
    fn boot_address(&self, access: Access<'_>, hart: Self::Secondary) -> u32;
}

/// Release a held secondary hart into `entry`, in ESP-IDF's
/// `start_other_core` order with the boot word last: request it to run and
/// confirm it is not stalled within `polls` reads, open its CPU clock, then its
/// CLIC clock, release its global reset, fence, write the boot word and fence.
/// The ROM's `main` path on the hart then jumps to `entry`.
///
/// SOURCE: ESP-IDF fff9895c `components/esp_system/port/cpu_start.c:308-348`
/// (`start_other_core`: unstall, clocks and reset, then
/// `ets_set_appcpu_boot_addr`) and
/// `components/hal/esp32s31/include/hal/cpu_utility_ll.h:40-76` (stall code
/// 0xFF and a stall-status wait; CPU clock, CLIC clock, then global reset
/// release); HW E1 of 2026-10-31 (oer-notes research/hw-hart1-entry-e1.md)
/// ran this order on S31 rev0.
///
/// # Errors
///
/// [`Unconfirmed`] when the hart still reports stalled: no clock, reset or
/// boot word was written.
pub fn release<R: LaunchRegisters>(
    registers: &mut R,
    hart: R::Secondary,
    entry: u32,
    polls: u32,
) -> Result<(), Unconfirmed> {
    run(registers, R::hart(hart), polls)?;
    registers.enable_cpu_clock(Access::new(), hart);
    registers.enable_clic_clock(Access::new(), hart);
    registers.release_reset(Access::new(), hart);
    registers.fence(Access::new());
    registers.set_boot_address(Access::new(), hart, entry);
    registers.fence(Access::new());
    Ok(())
}

/// A boot word that read back nonzero after its clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootWordKept {
    pub readback: u32,
}

/// Clear a released hart's boot word, fence and read it back as zero, so the
/// ROM cannot hand the hart its entry a second time.
///
/// # Errors
///
/// [`BootWordKept`] with the value read back.
pub fn retire_boot_address<R: LaunchRegisters>(
    registers: &mut R,
    hart: R::Secondary,
) -> Result<(), BootWordKept> {
    registers.set_boot_address(Access::new(), hart, 0);
    registers.fence(Access::new());
    match registers.boot_address(Access::new(), hart) {
        0 => Ok(()),
        readback => Err(BootWordKept { readback }),
    }
}

/// Invalid identities select no peer; their quarantined owner only clears ROM
/// entries before issuing the system reset.
pub fn clear_boot_addresses<R: Registers>(registers: &mut R) {
    for &hart in R::SECONDARIES {
        registers.clear_boot_address(Access::new(), hart);
    }
}

#[cfg(test)]
mod tests;
