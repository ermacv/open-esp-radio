//! Register ports for the bootstrap's SoC initialization, which `esp_hal::init` and the
//! ESP-IDF application startup did before the bootstrap owned the chip:
//! hart 1 contained, the three APM access-path filters opened, the LP TRNG
//! running with its health tests. The PMS words and PSRAM PMA7 are the
//! bundled bootloader's and stay as found.
//!
//! SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/mod.rs:22-141`
//! (`riscv_preinit`: the TRNG enable), `esp-hal/src/soc/esp32s31/cpu_control.rs:6-96`
//! (hart 1 stalled and held), `esp-hal/src/soc/mod.rs:81-565` (`init`: the
//! APM filters), `esp-hal/src/soc/esp32s31/trng.rs` (`rng_ll_enable`) and
//! ESP-IDF 4d80c1b2
//! `components/soc/esp32s31/register/soc/{lp_apm,hp_apm,hp_mem_apm}_reg.h`
//! (`FUNC_CTRL`: the masters' filter enables) and
//! `components/hal/esp32s31/include/hal/rng_ll.h:163-178`. The research
//! record of what the bootstrap must do and what the bootloader left is
//! `oer-notes/research/s31-soc-init.md`.

use crate::platform::{Hart, SecondaryHart, core_control::CoreControlRegisters};
use oer_esp32s31_pac::{
    hp_apm::HpApmFilter, hp_mem_apm::HpMemApmFilter, instances, lp_apm::LpApmFilter,
    lp_peri_clkrst::LpPericlkrstRngInit, trng::TrngInit,
};
use oer_espressif_boot::{
    Access,
    drivers::{access_control, cores, entropy},
};
use oer_mmio::{Bits, Fixed, Placement};

/// The SoC words the bootstrap initializes: the three APM filters, the LP
/// TRNG's configuration and clock word, and the core-control words that
/// contain hart 1.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Soc<
    P: Placement = Fixed<{ instances::lp_apm::BASE }>,
    Q: Placement = Fixed<{ instances::hp_apm::BASE }>,
    R: Placement = Fixed<{ instances::hp_mem_apm::BASE }>,
    S: Placement = Fixed<{ instances::trng::BASE }>,
    T: Placement = Fixed<{ instances::lp_periclkrst::BASE }>,
> {
    lp_apm: LpApmFilter<P>,
    hp_apm: HpApmFilter<Q>,
    hp_mem_apm: HpMemApmFilter<R>,
    trng: TrngInit<S>,
    rng_clock: LpPericlkrstRngInit<T>,
    cores: CoreControlRegisters,
}

impl Soc {
    /// Own the bootstrap's SoC parts and the core-control words.
    pub(crate) fn new(
        lp_apm: LpApmFilter,
        hp_apm: HpApmFilter,
        hp_mem_apm: HpMemApmFilter,
        trng: TrngInit,
        rng_clock: LpPericlkrstRngInit,
        cores: CoreControlRegisters,
    ) -> Self {
        Self {
            lp_apm,
            hp_apm,
            hp_mem_apm,
            trng,
            rng_clock,
            cores,
        }
    }
}
impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement> cores::Registers
    for Soc<P, Q, R, S, T>
{
    type Secondary = SecondaryHart;
    const SECONDARIES: &'static [SecondaryHart] = &Hart::SECONDARIES;
    fn request_stall(&mut self, _: Access<'_>, hart: SecondaryHart) {
        self.cores.request_stall(hart.hart());
    }
    fn hold(&mut self, _: Access<'_>, hart: SecondaryHart) {
        self.cores.hold(hart);
    }
    fn clear_boot_address(&mut self, _: Access<'_>, hart: SecondaryHart) {
        self.cores.set_boot_address(hart, 0);
    }
}
impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement> access_control::Registers
    for Soc<P, Q, R, S, T>
{
    fn open_lp_filter(&mut self, _: Access<'_>) {
        self.lp_apm.modify_func_ctrl(|w| {
            w.set_m0_func_en(false)
                .set_m1_func_en(false)
                .set_m2_func_en(false)
                .set_m3_func_en(false);
        });
    }
    fn open_hp_filter(&mut self, _: Access<'_>) {
        self.hp_apm.modify_func_ctrl(|w| {
            w.set_m0_func_en(false)
                .set_m1_func_en(false)
                .set_m2_func_en(false)
                .set_m3_func_en(false)
                .set_m4_func_en(false)
                .set_m5_func_en(false)
                .set_m6_func_en(false);
        });
    }
    fn open_memory_filter(&mut self, _: Access<'_>) {
        self.hp_mem_apm.modify_func_ctrl(|w| {
            w.set_m0_func_en(false)
                .set_m1_func_en(false)
                .set_m2_func_en(false)
                .set_m3_func_en(false)
                .set_m4_func_en(false)
                .set_m5_func_en(false);
        });
    }
}
impl<P: Placement, Q: Placement, R: Placement, S: Placement, T: Placement> entropy::Registers
    for Soc<P, Q, R, S, T>
{
    type Noise = Bits<5>;
    type Cutoff = Bits<9>;
    fn open_bus_clock(&mut self, _: Access<'_>) {
        self.rng_clock.modify_rng_ctrl(|w| {
            w.set_lp_rng_clk_en(true);
        });
    }
    fn open_clock(&mut self, _: Access<'_>) {
        self.trng.modify_date(|w| {
            w.set_clk_en(true);
        });
    }
    fn reset(&mut self, _: Access<'_>, asserted: bool) {
        self.rng_clock.modify_rng_ctrl(|w| {
            w.set_lp_rng_rst_en(asserted);
        });
    }
    fn configure_noise(
        &mut self,
        _: Access<'_>,
        profile: &entropy::Profile<Self::Noise, Self::Cutoff>,
    ) {
        self.trng.modify_conf(|w| {
            w.set_noise_source_sel(profile.noise_source)
                .set_noise_pos_sel(profile.noise_position)
                .set_repetition_value_c(profile.repetition_cutoff)
                .set_adpative_value_c(profile.adaptive_cutoff);
        });
    }
    fn configure_health_test(&mut self, _: Access<'_>, samples: u16) {
        self.trng.modify_debug_conf(|w| {
            w.set_startup_test_limit(samples)
                .set_health_test_bypass(false);
        });
    }
    fn enable_sampling(&mut self, _: Access<'_>) {
        self.trng.modify_conf(|w| {
            w.set_random_output_mode(true)
                .set_noise_crc_en(true)
                .set_sample_enable(true);
        });
    }
    fn start_health_test(&mut self, _: Access<'_>) {
        self.trng.trigger_debug_conf_startup_test_start();
    }
}
