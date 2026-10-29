//! The brownout detector's register port for the family driver.
//! The application policy: ESP-IDF's application replaces the
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
use oer_espressif_boot::{
    Access,
    drivers::brownout::{Mode0, Registers, Settings},
};
use oer_mmio::{Bits, Fixed, Placement};

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

impl<P: Placement> Registers for BrownoutRegisters<P> {
    type Wait = Bits<10>;
    type Address = analog_i2c::Address;
    fn configure_mode0(&mut self, _: Access<'_>, settings: Settings<Self::Wait>) {
        // brownout_ll_ana_reset_enable(false): take mode 1 into software
        // control, then disarm its reset.
        self.0.modify_fib_enable(|w| {
            w.set_bod_mode1_fib(false);
        });
        self.0.modify_bod_mode1_cntl(|w| {
            w.set_bod_mode1_reset_ena(false);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_intr_wait(settings.interrupt_wait);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_close_flash_ena(settings.close_flash);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_pd_rf_ena(settings.power_down_rf);
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
            w.set_bod_mode0_reset_wait(settings.reset_wait);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_reset_ena(settings.reset_enabled);
        });
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_reset_sel(settings.system_reset);
        });
    }

    fn enable(&mut self, _: Access<'_>) {
        self.0.modify_bod_mode0_cntl(|w| {
            w.set_bod_mode0_intr_ena(true);
        });
    }
    fn mode0(&self, _: Access<'_>) -> Mode0<Self::Wait> {
        let word = self.0.read_bod_mode0_cntl();
        Mode0 {
            enabled: word.bod_mode0_intr_ena(),
            settings: Settings {
                interrupt_wait: word.bod_mode0_intr_wait(),
                reset_wait: word.bod_mode0_reset_wait(),
                close_flash: word.bod_mode0_close_flash_ena(),
                power_down_rf: word.bod_mode0_pd_rf_ena(),
                reset_enabled: word.bod_mode0_reset_ena(),
                system_reset: word.bod_mode0_reset_sel(),
            },
        }
    }
    fn mode1_software(&self, _: Access<'_>) -> bool {
        !self.0.read_fib_enable().bod_mode1_fib()
    }
    fn mode1_reset(&self, _: Access<'_>) -> bool {
        self.0.read_bod_mode1_cntl().bod_mode1_reset_ena()
    }
    fn threshold_field() -> oer_espressif_analog::AnalogField<Self::Address> {
        analog_i2c::ulp_bod::i2c_bod_threshold::I2C_BOD_THRESHOLD
    }
}
#[cfg(any(test, feature = "test-support"))]
impl<'a> BrownoutRegisters<oer_mmio::Host<'a>> {
    /// The detector's words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpAnaBrownout::host(words).map(Self)
    }
}
