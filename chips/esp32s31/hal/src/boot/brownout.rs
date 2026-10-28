//! The brownout detector's policy: ESP-IDF's application replaces the
//! bootloader's analog mode-1 reset with the software-controlled mode-0
//! detector, once, before the application runs, and the boot stops unless
//! the detector reads back in that policy.
//!
//! SOURCE: ESP-IDF 4d59230d `esp_brownout_init`
//! (`components/esp_hw_support/power_supply/brownout.c`), run at startup by
//! `init_brownout` (`components/esp_system/startup_funcs.c`), in the
//! register order of `brownout_hal_config`
//! (`components/esp_hal_pmu/brownout_hal.c`,
//! `components/esp_hal_pmu/esp32s31/include/hal/brownout_ll.h`). The
//! ESP32-S31 has no `ESP_BROWNOUT_USE_INTR`, so it configures mode 0 with
//! the hardware reset enabled, flash and RF power-down, at
//! `ESP_BROWNOUT_DET_LVL` 7 (about 2.4 V), the S31 default of
//! `components/esp_hw_support/power_supply/port/esp32s31/Kconfig.power`;
//! esp-hal 1f646c5 `esp-hal/src/rtc_cntl/brownout.rs` (`configure`) follows
//! the same order. The interrupt enables (`LP_ANA.INT_ENA`) are left as
//! found: no handler is installed.

use oer_esp32s31_pac::{analog_i2c, instances, lp_ana::LpAnaBrownout};
use oer_espressif_analog::{AnalogRegisterBus, FieldWrite, Step};
use oer_espressif_boot::wait::{BootDeadline, BootWait, WaitFailure};
use oer_mmio::{Bits, Fixed, Placement};
use oer_time::{Clock, Duration};

#[cfg(test)]
mod tests;

/// Brownout reset wait, in detector cycles (ESP-IDF's fixed `0x3ff`).
const RESET_WAIT: Bits<10> = Bits::constant::<0x3ff>();

/// Detector cycles before the brownout event (ESP-IDF's fixed `2`).
const INTR_WAIT: Bits<10> = Bits::constant::<2>();

/// The detector's threshold level, `ESP_BROWNOUT_DET_LVL` 7 (about 2.4 V).
const THRESHOLD: u8 = 7;

/// One analog-I2C byte transaction of the threshold's write: a few bus
/// clocks at the master's rate. ESP-IDF's `regi2c_write_mask` polls the
/// host's busy bit without a limit; this bound is this crate's.
pub const THRESHOLD_WAIT: BootWait = const {
    match BootWait::new("bod-threshold", Duration::from_millis(1)) {
        Ok(wait) => wait,
        Err(_) => panic!("a boot wait has a nonzero bound"),
    }
};

/// Why the policy was not applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrownoutFailure {
    /// The threshold's analog-I2C write did not complete within its wait.
    Threshold(WaitFailure),
    /// The detector does not read back in the policy.
    Readback,
    Transition(oer_espressif_analog::TransitionError),
}

/// The brownout detector's control words.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct BrownoutRegisters<P: Placement = Fixed<{ instances::lp_ana_peri::BASE }>>(
    LpAnaBrownout<P>,
);

impl BrownoutRegisters {
    /// Own the bootstrap's brownout part.
    pub(crate) fn new(part: LpAnaBrownout) -> Self {
        Self(part)
    }
}

impl<P: Placement> BrownoutRegisters<P> {
    /// Take mode 1 into software control, disarm its reset, then configure
    /// mode 0's waits and actions, clear its count and arm its system
    /// reset: everything before the threshold.
    fn configure_mode0(&mut self) {
        // brownout_ll_ana_reset_enable(false): take mode 1 into software
        // control, then disarm its reset.
        self.0.modify_fib_enable(|w| {
            w.set_bod_mode1_fib(false);
        });
        self.0.modify_bod_mode1_cntl(|w| {
            w.set_bod_mode1_reset_ena(false);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_intr_wait(INTR_WAIT);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_close_flash_ena(true);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_pd_rf_ena(true);
        });
        // brownout_ll_clear_count
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_cnt_clr(true);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_cnt_clr(false);
        });
        // brownout_ll_reset_config(true, 0x3ff, BROWNOUT_RESET_LEVEL_SYSTEM)
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_reset_wait(RESET_WAIT);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_reset_ena(true);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_reset_sel(true);
        });
    }

    /// brownout_ll_bod_enable(true): arm mode 0's event.
    fn enable(&mut self) {
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_intr_ena(true);
        });
    }

    /// Whether the detector reads back in the policy: mode 1 under software
    /// control with its reset disarmed, mode 0 armed with its actions and
    /// waits. The threshold is an analog register the check does not read.
    fn in_policy(&self) -> bool {
        let mode0 = self.0.read_bod_mode0_cntl();
        !self.0.read_fib_enable().bod_mode1_fib()
            && !self.0.read_bod_mode1_cntl().bod_mode1_reset_ena()
            && mode0.bod_mode0_reset_ena()
            && mode0.bod_mode0_reset_sel()
            && mode0.bod_mode0_intr_ena()
            && mode0.bod_mode0_pd_rf_ena()
            && mode0.bod_mode0_close_flash_ena()
            && mode0.bod_mode0_reset_wait() == RESET_WAIT
            && mode0.bod_mode0_intr_wait() == INTR_WAIT
    }
}

/// Apply ESP-IDF's application brownout policy over `registers` and the
/// analog bus `bus`, under `deadline`, then check it reads back.
///
/// # Errors
///
/// The threshold's write did not complete in its wait or before the
/// deadline, or the detector does not read back in the policy.
pub fn apply_idf_policy<P, B, C>(
    registers: &mut BrownoutRegisters<P>,
    bus: &mut B,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BrownoutFailure>
where
    P: Placement,
    B: AnalogRegisterBus<Address = analog_i2c::Address>,
    C: Clock + ?Sized,
{
    registers.configure_mode0();
    let threshold = analog_i2c::ulp_bod::i2c_bod_threshold::I2C_BOD_THRESHOLD;
    let mut write = match FieldWrite::new(bus, threshold, THRESHOLD) {
        Ok(write) => write,
        Err(_) => unreachable!("the threshold level fits its three-bit field"),
    };
    deadline
        .poll(THRESHOLD_WAIT, || match write.poll() {
            Ok(Step::Ready(())) => Some(Ok(())),
            Ok(Step::Pending) => None,
            Err(error) => Some(Err(BrownoutFailure::Transition(error))),
        })
        .map_err(BrownoutFailure::Threshold)??;
    registers.enable();
    if registers.in_policy() {
        Ok(())
    } else {
        Err(BrownoutFailure::Readback)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> BrownoutRegisters<oer_mmio::Host<'a>> {
    /// The detector's words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpAnaBrownout::host(words).map(Self)
    }
}
