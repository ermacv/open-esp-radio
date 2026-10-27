//! TIMG0 T0's relative one-shot alarm with disjoint program and IRQ owners.
//! The actor moves the program owner in its transaction bundle; the handler
//! retains the interrupt owner for the image's lifetime. No state or lock is shared.
//!
//! Runtime startup selects/enables XTAL through the watchdog's retained
//! clock/reset word. Adopting these parts performs no hardware writes.
//! The SAR/PWDET pilot and the host composition fixture select P2
//! `Exempt(ExecutorLevel)` and a lower line ID than FROM_CPU2. Its arbitration
//! interpretation follows the family CLIC contract:
//! SOURCE: chips/espressif/platform/family.toml [interrupts], RISC-V CLIC
//! specification 3f8ee744, "Specifying Interrupt Priority"; ESP-IDF 4d59230d
//! components/soc/esp32s31/include/soc/clic_reg.h (NLBITS/INT_INFO geometry).
//! SOURCE for same-level priority/ID arbitration: ESP32-C5 TRM v1.1,
//! documentation/esp32-c5_technical_reference_manual_en.pdf, section 2.8.2.5,
//! p.86. SOURCE: HW23_S31_P2_ARBITRATION_20261027 confirms line17 before16
//! on S31 hart0 at P2, with TIMG0/FROM_CPU2 assigned both ways. Other line
//! pairs and levels are not measured by that observation.

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

/// T0 RAW followed by ST, not an atomic snapshot or authority for another lane.
#[must_use = "retain the pending observation acknowledged by this operation"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    pub pending: bool,
    pub asserted: bool,
}

/// Independent owners: the program part travels in the actor's transaction
/// bundle; only the interrupt part is permanently bound to the handler.
pub struct Timg0AlarmParts {
    pub program: Timg0AlarmProgram,
    pub interrupt: Timg0AlarmInterrupt,
}

impl Timg0AlarmParts {
    /// Adopt the root's parts without enabling, arming or acknowledging T0.
    pub(crate) fn new(parts: TimerGroup0Peripherals) -> Self {
        Self {
            program: Timg0AlarmProgram(parts.timg0_timer0),
            interrupt: Timg0AlarmInterrupt(parts.timg0_interrupt_bank),
        }
    }
}

/// T0 configuration, alarm, load and the whole interrupt-enable word.
/// This affine owner moves between actor and handler; it cannot acknowledge.
#[must_use = "dropping the program part permanently loses its register authority"]
pub struct Timg0AlarmProgram<P: Placement = TimerPlacement>(timg::Timg0Timer0<P>);

/// The group's RAW/ST words and only T0's CLR strobe. This affine owner
/// stays in the handler for the image's lifetime; it cannot program or enable.
#[must_use = "the interrupt table retains this owner for the image's lifetime"]
pub struct Timg0AlarmInterrupt<P: Placement = TimerPlacement>(timg::Timg0InterruptBank<P>);

impl<P: Placement> InterruptSource for Timg0AlarmInterrupt<P> {
    // SOURCE: generated TIMERGRP0_T0/source25, ESP-IDF 4d59230d interrupt
    // facts and HW7_S31_TIMG0_20261026 in the register model.
    const SOURCE: u16 = <timg::Timg0InterruptBank<P> as InterruptSource>::SOURCE;
}

impl<P: Placement> Timg0AlarmProgram<P> {
    /// Replace the old target with a positive interval counted from a fresh zero.
    /// Stops and masks, loads zero and the target, starts, then unmasks.
    /// Does not acknowledge pending: the handler uses its interrupt part for that.
    /// The actor can arm before depositing the bundle; the handler can rearm
    /// after acknowledging its event, without lending its permanent IRQ owner.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// components/esp_hal_timg/esp32s31/include/hal/timer_ll.h:293-321
    /// (reload value and software reload), :236-239 (counter enable), :279-283
    /// (alarm value). SOURCE: HW23_S31_TIMG0_RELOAD_20261027 confirms the
    /// stop/load-zero/strobe/start sequence on S31 at XTAL/divider40 for
    /// 10/100/1000 us and with old pending retained across rearm. A separate
    /// acknowledgement is still required; arm does not consume old pending.
    /// HW7_S31_TIMG0_20261026 qualifies one-shot expiry, masked RAW retention
    /// and ENA redelivery, not clear/arrival collisions or an IRQ latency bound.
    ///
    /// # Errors
    /// [`InvalidDelay`] before any mutation, preserving the previous alarm.
    pub fn arm_after(
        &mut self,
        delay: Duration,
        _authority: &impl EventAuthority<Timg0AlarmInterrupt<P>>,
    ) -> Result<(), InvalidDelay> {
        arm_after(self, delay)
    }

    /// Stop and mask T0 without acknowledging RAW. Pending remains available
    /// to the handler's interrupt part even though ST becomes zero.
    /// SOURCE: HW7_S31_TIMG0_20261026, stop/mask/CLR phases.
    pub fn disarm(&mut self) {
        disarm(self);
    }

    /// Whether T0's peripheral delivery lane is enabled.
    pub fn enabled(&self) -> bool {
        self.0.read_int_ena_timers().t0_int_ena()
    }
}

impl<P: Placement> Timg0AlarmInterrupt<P> {
    /// Observe RAW, then ST, and acknowledge a sampled pending T0 through CLR.
    /// Neither ENA nor counter/alarm programming changes. Expiry self-clears
    /// alarm enable; this operation never rearms. A spurious entry with RAW zero
    /// issues no clear. Masking does not hide RAW from this owner.
    /// SOURCE: HW7_S31_TIMG0_20261026, repeated pending and CLR at ENA=0/1.
    pub fn acknowledge(&mut self) -> Observation {
        acknowledge(self)
    }

    /// Read T0 RAW/ST, including pending hidden by masking.
    pub fn observe(&self) -> Observation {
        InterruptRegisters::observe(self)
    }
}

// Separate private contracts preserve the production ownership split in the
// virtual hardware model. Neither backend nor PAC access escapes into protocols.
trait ProgramRegisters {
    fn stop(&mut self);
    fn enable(&mut self, enabled: bool);
    fn load_interval(&mut self, micros: u64);
    fn start(&mut self);
}

trait InterruptRegisters {
    fn observe(&self) -> Observation;
    fn clear_timer0(&mut self);
}

fn arm_after(registers: &mut impl ProgramRegisters, delay: Duration) -> Result<(), InvalidDelay> {
    let micros = delay.as_micros();
    if micros == 0 || micros > MAX_DELAY_MICROS {
        return Err(InvalidDelay { requested: delay });
    }
    disarm(registers);
    registers.load_interval(micros);
    // An expiry under the mask remains in RAW for ENA to deliver (HW7).
    registers.start();
    oer_riscv::fence();
    registers.enable(true);
    oer_riscv::fence();
    Ok(())
}

fn disarm(registers: &mut impl ProgramRegisters) {
    registers.stop();
    registers.enable(false);
    oer_riscv::fence();
}

fn acknowledge(registers: &mut impl InterruptRegisters) -> Observation {
    let observation = registers.observe();
    if observation.pending {
        registers.clear_timer0();
        oer_riscv::fence();
    }
    observation
}

impl<P: Placement> ProgramRegisters for Timg0AlarmProgram<P> {
    fn stop(&mut self) {
        self.0.configure_timg0_timer0_stopped();
    }

    fn enable(&mut self, enabled: bool) {
        // Preserve T1, WDT, reserved and unknown lanes. No RAW/CLR authority.
        self.0.modify_int_ena_timers(|w| {
            w.set_t0_int_ena(enabled);
        });
    }

    fn load_interval(&mut self, micros: u64) {
        // Both supported targets are little-endian. Admission above proves
        // the high word fits the generated field before Bits::truncate.
        let [low, high]: [u32; 2] = bytemuck::cast(micros);
        self.0.modify_t0alarmlo(|w| {
            w.set_t_alarm_lo(low);
        });
        self.0.modify_t0alarmhi(|w| {
            w.set_t_alarm_hi(Bits::truncate(high));
        });
        self.0.modify_t0loadlo(|w| {
            w.set_t_load_lo(0);
        });
        self.0.modify_t0loadhi(|w| {
            w.set_t_load_hi(Bits::constant::<0>());
        });
        // SOURCE: ESP-IDF 4d59230d components/esp_hal_timg/esp32s31/include/
        // hal/timer_ll.h:293-321 (set_reload_value / trigger_soft_reload).
        // SOURCE: HW23_S31_TIMG0_RELOAD_20261027 observes both counter halves
        // reload to zero and 10/100/1000-us expiry after this sequence on S31.
        self.0.trigger_t0load_t_load(1);
    }

    fn start(&mut self) {
        // SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL, sourced fixed image:
        // increment, divider40 plus divider reset, no autoreload, EN+ALARM.
        self.0.configure_timg0_timer0_armed();
    }
}

impl<P: Placement> InterruptRegisters for Timg0AlarmInterrupt<P> {
    fn observe(&self) -> Observation {
        Observation {
            pending: self.0.read_int_raw_timers().t0_int_raw(),
            asserted: self.0.read_int_st_timers().t0_int_st(),
        }
    }

    fn clear_timer0(&mut self) {
        self.0.trigger_int_clr_timers_t0_int_clr();
    }
}
