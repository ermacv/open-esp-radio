//! TIMG0 T0's relative one-shot alarm. One handler retains both the counter
//! and the whole interrupt bank; no task shares its register writer.
//!
//! Runtime startup selects/enables XTAL through the watchdog's retained
//! clock/reset word. Constructing this owner performs no hardware writes.
//! No image binds or arms it. An executor-level consumer binds it as
//! `Exempt(ExecutorLevel, "PHY steps between executor rounds")`, below
//! FROM_CPU2 in same-level arbitration; handler/actor exchange belongs to
//! that image's composition.

use oer_esp32s31_pac::{instances, ownership::TimerGroup0Peripherals, timg};
use oer_interrupt_table::EventAuthority;
use oer_mmio::{Bits, Fixed, InterruptSource, Placement};
use oer_time::Duration;

#[cfg(test)]
mod tests;

/// Where timer group zero lies.
pub type TimerPlacement = Fixed<{ instances::timg0::BASE }>;

/// Largest relative delay at the configured one tick per microsecond.
/// Geometry comes from the generated alarm fields.
pub const MAX_DELAY_MICROS: u64 = ((timg::t0alarmhi::T_ALARM_HI_MAX as u64)
    << timg::t0alarmlo::T_ALARM_LO_WIDTH)
    | timg::t0alarmlo::T_ALARM_LO_MAX as u64;

/// The requested interval is zero or cannot fit the hardware counter. No write occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidDelay {
    pub requested: Duration,
}

/// A T0-only observation, in RAW, ENA, ST read order, not an atomic snapshot.
/// It grants no authority to clear any other interrupt-bank lane.
#[must_use = "retain the pending observation acknowledged by this operation"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    pub pending: bool,
    pub enabled: bool,
    pub asserted: bool,
}

/// The counter/alarm and its interrupt bank, retained together for the
/// handler's lifetime. All transitions require exclusive mutable access.
#[must_use = "dropping the alarm permanently loses its register authority"]
pub struct Timg0Alarm<P: Placement = TimerPlacement> {
    registers: AlarmRegisters<P>,
}

struct AlarmRegisters<P: Placement> {
    timer: timg::Timg0Timer0<P>,
    bank: timg::Timg0InterruptBank<P>,
}

impl Timg0Alarm {
    /// Adopt the root's parts without enabling, arming or acknowledging T0.
    pub(crate) fn new(parts: TimerGroup0Peripherals) -> Self {
        Self {
            registers: AlarmRegisters {
                timer: parts.timg0_timer0,
                bank: parts.timg0_interrupt_bank,
            },
        }
    }
}

impl<P: Placement> InterruptSource for Timg0Alarm<P> {
    // SOURCE: generated TIMERGRP0_T0/source25, ESP-IDF 4d59230d interrupt
    // facts and HW7_S31_TIMG0_20261026 in the register model.
    const SOURCE: u16 = <timg::Timg0InterruptBank<P> as InterruptSource>::SOURCE;
}

impl<P: Placement> Timg0Alarm<P> {
    /// Replace the old alarm with an interval counted from a fresh zero.
    /// Returns the old pending observation, acknowledged before rearming.
    /// Intervals are positive; no absolute/wrapping epoch is used.
    /// Equality at a freshly reloaded zero is not qualified by the stand facts.
    /// The router's proof authorizes enabling this owner's interrupt only.
    ///
    /// SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL (increment/no-autoreload,
    /// XTAL/divider40 = nominal 1 MHz; software load); HW7_S31_TIMG0_20261026
    /// (one-shot, masked RAW retention and CLR independent of ENA).
    /// No clear/arrival collision or arm-to-IRQ latency bound is assumed.
    ///
    /// # Errors
    /// [`InvalidDelay`] before any mutation, preserving the previous alarm.
    pub fn arm_after(
        &mut self,
        delay: Duration,
        _authority: &impl EventAuthority<Self>,
    ) -> Result<Observation, InvalidDelay> {
        arm_after(&mut self.registers, delay)
    }

    /// Stop the producer, mask delivery, observe RAW even under the mask,
    /// then clear the observed T0 cause only. Returns the consumed observation.
    /// Stopping/masking alone never acknowledges pending; the group clock and
    /// watchdog remain untouched. This owner stays retained, not reusable.
    ///
    /// SOURCE: HW7_S31_TIMG0_20261026, stop/mask/CLR phases.
    pub fn disarm(&mut self) -> Observation {
        disarm(&mut self.registers)
    }

    /// Observe then acknowledge T0 through CLR, leaving ENA and the counter
    /// unchanged. Expiry self-clears alarm enable, so this never rearms it.
    /// A spurious entry with RAW zero issues no clear.
    ///
    /// SOURCE: HW7_S31_TIMG0_20261026, repeated pending, CLR and one-shot phases.
    pub fn acknowledge(&mut self) -> Observation {
        acknowledge(&mut self.registers)
    }

    /// Read T0's RAW/ENA/ST, including a pending cause hidden by masking.
    pub fn observe(&self) -> Observation {
        self.registers.observe()
    }
}

// Register sequencing is private; neither a backend nor PAC access escapes
// into protocol code. Host tests model asynchronous producer/IRQ side effects.
trait Registers {
    fn stop(&mut self);
    fn enable(&mut self, enabled: bool);
    fn observe(&self) -> Observation;
    fn clear(&mut self);
    fn load_interval(&mut self, micros: u64);
    fn start(&mut self);
}

fn arm_after(registers: &mut impl Registers, delay: Duration) -> Result<Observation, InvalidDelay> {
    let micros = delay.as_micros();
    if micros == 0 || micros > MAX_DELAY_MICROS {
        return Err(InvalidDelay { requested: delay });
    }
    let previous = disarm(registers);
    registers.load_interval(micros);
    // Start under the mask: an immediate expiry remains in RAW and is
    // delivered by the following ENA write (HW7_S31_TIMG0_20261026).
    registers.start();
    oer_riscv::fence();
    registers.enable(true);
    oer_riscv::fence();
    Ok(previous)
}

fn disarm(registers: &mut impl Registers) -> Observation {
    registers.stop();
    registers.enable(false);
    oer_riscv::fence();
    acknowledge(registers)
}

fn acknowledge(registers: &mut impl Registers) -> Observation {
    let observation = registers.observe();
    if observation.pending {
        registers.clear();
        oer_riscv::fence();
    }
    observation
}

impl<P: Placement> Registers for AlarmRegisters<P> {
    fn stop(&mut self) {
        self.timer.configure_timg0_timer0_stopped();
    }

    fn enable(&mut self, enabled: bool) {
        // Preserve T1, WDT, reserved and unknown lanes. RAW is never written.
        self.bank.modify_int_ena_timers(|w| {
            w.set_t0_int_ena(enabled);
        });
    }

    fn observe(&self) -> Observation {
        Observation {
            pending: self.bank.read_int_raw_timers().t0_int_raw(),
            enabled: self.bank.read_int_ena_timers().t0_int_ena(),
            asserted: self.bank.read_int_st_timers().t0_int_st(),
        }
    }

    fn clear(&mut self) {
        self.bank.trigger_int_clr_timers_t0_int_clr();
    }

    fn load_interval(&mut self, micros: u64) {
        // Both supported targets are little-endian. Admission above proves
        // the high word fits the generated field before Bits::truncate.
        let [low, high]: [u32; 2] = bytemuck::cast(micros);
        self.timer.modify_t0alarmlo(|w| {
            w.set_t_alarm_lo(low);
        });
        self.timer.modify_t0alarmhi(|w| {
            w.set_t_alarm_hi(Bits::truncate(high));
        });
        self.timer.modify_t0loadlo(|w| {
            w.set_t_load_lo(0);
        });
        self.timer.modify_t0loadhi(|w| {
            w.set_t_load_hi(Bits::constant::<0>());
        });
        // SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL, timer_ll_set_reload_value /
        // timer_ll_trigger_soft_reload: any LOAD store strobes the reload.
        self.timer.trigger_t0load_t_load(1);
    }

    fn start(&mut self) {
        // SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL, sourced fixed image:
        // increment, divider40 plus divider reset, no autoreload, EN+ALARM.
        self.timer.configure_timg0_timer0_armed();
    }
}
