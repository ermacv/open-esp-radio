//! TIMG1 T0's relative one-shot alarm with disjoint program and IRQ owners,
//! the same operations as [`timg0_alarm`](super::timg0_alarm) on timer group
//! one's words. No product timer uses it; a HIL image's PC sampler does.
//! The program part moves with whoever arms it; the handler retains the
//! interrupt part for the image's lifetime. No state or lock is shared.
//!
//! Runtime startup selects/enables XTAL for T0 through the watchdog's
//! retained clock/reset word (group one's watchdog is the system deadline's).
//! Adopting these parts performs no hardware writes.
//!
//! SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL in the register model: the
//! ESP-IDF timer interface takes the group as a parameter, so the T0
//! sequences and fixed configurations are the same for both groups. The
//! TIMG0 stand observations (HW7, HW23) were not repeated on TIMG1.

use super::timg0_alarm::{
    InterruptRegisters, InvalidDelay, MAX_DELAY_MICROS, Observation, ProgramRegisters, acknowledge,
    arm_after, disarm,
};
use oer_esp32s31_pac::{instances, ownership::TimerGroup1Peripherals, timg};
use oer_espressif_interrupt_table::EventAuthority;
use oer_mmio::{Bits, Fixed, InterruptSource, Placement};
use oer_time::Duration;

/// Where timer group one lies.
pub type TimerPlacement = Fixed<{ instances::timg1::BASE }>;

/// Independent owners: the program part moves with whoever arms the alarm;
/// only the interrupt part is permanently bound to the handler.
pub struct Timg1AlarmParts {
    pub program: Timg1AlarmProgram,
    pub interrupt: Timg1AlarmInterrupt,
}

impl Timg1AlarmParts {
    /// Adopt the root's parts without enabling, arming or acknowledging T0.
    pub(crate) fn new(parts: TimerGroup1Peripherals) -> Self {
        Self {
            program: Timg1AlarmProgram(parts.timg1_timer0),
            interrupt: Timg1AlarmInterrupt(parts.timg1_interrupt_bank),
        }
    }
}

/// T0 configuration, alarm, load and the whole interrupt-enable word.
/// This affine owner can move to the handler; it cannot acknowledge.
#[must_use = "dropping the program part permanently loses its register authority"]
pub struct Timg1AlarmProgram<P: Placement = TimerPlacement>(timg::Timg1Timer0<P>);

/// The group's RAW/ST words and only T0's CLR strobe. This affine owner
/// stays in the handler for the image's lifetime; it cannot program or enable.
#[must_use = "the interrupt table retains this owner for the image's lifetime"]
pub struct Timg1AlarmInterrupt<P: Placement = TimerPlacement>(timg::Timg1InterruptBank<P>);

impl<P: Placement> InterruptSource for Timg1AlarmInterrupt<P> {
    // SOURCE: generated TIMERGRP1_T0/source28 and ESP-IDF 4d80c1b2
    // interrupt facts in the register model.
    const SOURCE: u16 = <timg::Timg1InterruptBank<P> as InterruptSource>::SOURCE;
}

impl<P: Placement> Timg1AlarmProgram<P> {
    /// Replace the old target with a positive interval counted from a fresh
    /// zero: stop and mask, load zero and the target, start, then unmask.
    /// Does not acknowledge pending: the handler's interrupt part does that.
    ///
    /// # Errors
    /// [`InvalidDelay`] before any mutation, preserving the previous alarm.
    pub fn arm_after(
        &mut self,
        delay: Duration,
        _authority: &impl EventAuthority<Timg1AlarmInterrupt<P>>,
    ) -> Result<(), InvalidDelay> {
        arm_after(self, delay)
    }

    /// Move the alarm of the counter [`arm_after`](Self::arm_after) started
    /// to `target` microseconds from that zero and enable it again, without
    /// stopping, reloading or resetting the divider: a periodic schedule
    /// keeps its phase whatever the handler's latency. A target the counter
    /// already passed fires at once.
    ///
    /// SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL (`timer_ll_set_alarm_value`,
    /// `timer_ll_enable_alarm` on a counting timer); HW7_S31_TIMG0_20261026
    /// observed a past target firing on TIMG0, not repeated on TIMG1.
    ///
    /// # Errors
    /// [`InvalidDelay`] for a target past the counter's range, before any
    /// write.
    pub fn alarm_at(
        &mut self,
        target: Duration,
        _authority: &impl EventAuthority<Timg1AlarmInterrupt<P>>,
    ) -> Result<(), InvalidDelay> {
        alarm_at(self, target)
    }

    /// Stop and mask T0 without acknowledging RAW.
    pub fn disarm(&mut self) {
        disarm(self);
    }

    /// Whether T0's peripheral delivery lane is enabled.
    pub fn enabled(&self) -> bool {
        self.0.read_int_ena_timers().t0_int_ena()
    }
}

impl<P: Placement> Timg1AlarmInterrupt<P> {
    /// Observe RAW, then ST, and acknowledge a sampled pending T0 through CLR.
    /// Neither ENA nor counter/alarm programming changes; this never rearms.
    /// A spurious entry with RAW zero issues no clear.
    pub fn acknowledge(&mut self) -> Observation {
        acknowledge(self)
    }

    /// Read T0 RAW/ST, including pending hidden by masking.
    pub fn observe(&self) -> Observation {
        InterruptRegisters::observe(self)
    }
}

fn alarm_at<P: Placement>(
    program: &mut Timg1AlarmProgram<P>,
    target: Duration,
) -> Result<(), InvalidDelay> {
    let micros = target.as_micros();
    if micros > MAX_DELAY_MICROS {
        return Err(InvalidDelay { requested: target });
    }
    program.set_alarm(micros);
    program.0.configure_timg1_timer0_alarm_running();
    oer_riscv::fence();
    Ok(())
}

impl<P: Placement> Timg1AlarmProgram<P> {
    /// The alarm words; the caller proved `micros` fits them.
    fn set_alarm(&mut self, micros: u64) {
        // Both supported targets are little-endian; Bits::truncate keeps
        // the high word's generated width, which the admission fits.
        let [low, high]: [u32; 2] = bytemuck::cast(micros);
        self.0.modify_t0alarmlo(|w| {
            w.set_t_alarm_lo(low);
        });
        self.0.modify_t0alarmhi(|w| {
            w.set_t_alarm_hi(Bits::truncate(high));
        });
    }
}

impl<P: Placement> ProgramRegisters for Timg1AlarmProgram<P> {
    fn stop(&mut self) {
        self.0.configure_timg1_timer0_stopped();
    }

    fn enable(&mut self, enabled: bool) {
        // Preserve T1, WDT, reserved and unknown lanes. No RAW/CLR authority.
        self.0.modify_int_ena_timers(|w| {
            w.set_t0_int_ena(enabled);
        });
    }

    fn load_interval(&mut self, micros: u64) {
        // Admission in `arm_after` proves the target fits the alarm words.
        self.set_alarm(micros);
        self.0.modify_t0loadlo(|w| {
            w.set_t_load_lo(0);
        });
        self.0.modify_t0loadhi(|w| {
            w.set_t_load_hi(Bits::constant::<0>());
        });
        // SOURCE: ESP-IDF 4d80c1b2 components/esp_hal_timg/esp32s31/include/
        // hal/timer_ll.h:293-321 (set_reload_value / trigger_soft_reload).
        self.0.trigger_t0load_t_load(1);
    }

    fn start(&mut self) {
        // SOURCE: ESP_IDF_4D59230D_S31_TIMER0_LL, sourced fixed image:
        // increment, divider40 plus divider reset, no autoreload, EN+ALARM.
        self.0.configure_timg1_timer0_armed();
    }
}

impl<P: Placement> InterruptRegisters for Timg1AlarmInterrupt<P> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn program(words: &mut [u32]) -> Timg1AlarmProgram<oer_mmio::Host<'_>> {
        Timg1AlarmProgram(timg::Timg1Timer0::host(words).expect("host memory holds the block"))
    }

    #[test]
    fn a_running_alarm_moves_its_target_and_reenables_it_without_a_reload() {
        let mut words = [0; instances::timg1::SIZE / 4];
        let mut timer = program(&mut words);
        timer.0.configure_timg1_timer0_armed();
        timer.0.modify_t0loadlo(|w| {
            w.set_t_load_lo(7);
        });
        alarm_at(&mut timer, Duration::from_micros(0x1_2345_6789)).expect("the target fits");
        assert_eq!(timer.0.read_t0alarmlo().t_alarm_lo(), 0x2345_6789);
        assert_eq!(timer.0.read_t0alarmhi().t_alarm_hi().get(), 1);
        let config = timer.0.read_t0config();
        assert!(config.t_en() && config.t_increase() && config.t_alarm_en());
        assert!(!config.t_autoreload());
        assert_eq!(config.t_divider(), 40, "the divider stays");
        assert_eq!(
            timer.0.read_t0loadlo().t_load_lo(),
            7,
            "no reload value is written"
        );
    }

    #[test]
    fn a_target_past_the_counter_s_range_writes_nothing() {
        let mut words = [0; instances::timg1::SIZE / 4];
        let before = words;
        let mut timer = program(&mut words);
        let target = Duration::from_micros(MAX_DELAY_MICROS + 1);
        assert_eq!(
            alarm_at(&mut timer, target),
            Err(InvalidDelay { requested: target })
        );
        drop(timer);
        assert_eq!(words, before);
    }
}
