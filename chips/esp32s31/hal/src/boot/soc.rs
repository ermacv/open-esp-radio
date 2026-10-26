//! The bootstrap's SoC initialization, which `esp_hal::init` and the
//! ESP-IDF application startup did before the bootstrap owned the chip:
//! hart 1 contained, the three APM access-path filters opened, the LP TRNG
//! running with its health tests. The PMS words and PSRAM PMA7 are the
//! bundled bootloader's and stay as found.
//!
//! SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/mod.rs:22-141`
//! (`riscv_preinit`: the TRNG enable), `esp-hal/src/soc/esp32s31/cpu_control.rs:6-96`
//! (hart 1 stalled and held), `esp-hal/src/soc/mod.rs:81-565` (`init`: the
//! APM filters), `esp-hal/src/soc/esp32s31/trng.rs` (`rng_ll_enable`) and
//! ESP-IDF 4d59230d
//! `components/soc/esp32s31/register/soc/{lp_apm,hp_apm,hp_mem_apm}_reg.h`
//! (`FUNC_CTRL`: the masters' filter enables) and
//! `components/hal/esp32s31/include/hal/rng_ll.h:163-178`. The research
//! record of what the bootstrap must do and what the bootloader left is
//! `oer-notes/research/s31-soc-init.md`.

use oer_esp32s31_pac::{
    hp_apm::HpApmFilter, hp_mem_apm::HpMemApmFilter, instances, lp_apm::LpApmFilter,
    lp_peri_clkrst::LpPericlkrstRngInit, trng::TrngInit,
};
use oer_espressif_boot::wait::{BootDeadline, WaitFailure};
use oer_mmio::{Bits, Fixed, Placement};
use oer_time::{Clock, Duration};

use crate::{cores, platform::core_control::CoreControlRegisters};

#[cfg(test)]
mod tests;

/// The TRNG's noise source and position selectors.
/// SOURCE: ESP-IDF 4d59230d `rng_ll.h:163-178` (`rng_ll_enable`: 0x10, 0x10).
const NOISE_SELECT: Bits<5> = Bits::constant::<0x10>();
/// The repetition-count health test's cutoff.
/// SOURCE: ESP-IDF 4d59230d `rng_ll.h:163-178` (0x1f).
const REPETITION_CUTOFF: Bits<9> = Bits::constant::<0x1f>();
/// The adaptive-proportion health test's cutoff.
/// SOURCE: ESP-IDF 4d59230d `rng_ll.h:163-178` (0x12).
const ADAPTIVE_CUTOFF: Bits<9> = Bits::constant::<0x12>();
/// The startup health test's sample count.
/// SOURCE: ESP-IDF 4d59230d `rng_ll.h:163-178` (1024).
const STARTUP_TEST_LIMIT: u16 = 1024;

/// Why the SoC initialization stopped: a typed boot failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocFailure {
    /// The boot deadline passed before the initialization.
    Deadline(WaitFailure),
}

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

    /// Contain hart 1, open the APM filters and start the TRNG, under
    /// `deadline`: no step waits.
    ///
    /// # Errors
    ///
    /// The boot deadline passed before the initialization began.
    pub fn initialize<C: Clock + ?Sized>(
        &mut self,
        deadline: &BootDeadline<'_, C>,
    ) -> Result<(), SocFailure> {
        if deadline.remaining() == Duration::ZERO {
            return Err(SocFailure::Deadline(WaitFailure::DeadlineExceeded(
                "soc-init",
            )));
        }
        let Self {
            lp_apm,
            hp_apm,
            hp_mem_apm,
            trng,
            rng_clock,
            cores,
        } = self;
        contain_secondaries(cores);
        open_apm_filters(lp_apm, hp_apm, hp_mem_apm);
        start_trng(trng, rng_clock);
        Ok(())
    }
}

/// Request hart 1's stall, gate its clocks and hold its reset, then clear
/// its ROM boot address, so the ROM it may wait in finds no stale entry.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/cpu_control.rs:6-96`;
/// ESP-IDF 4d59230d `components/esp_system/port/soc/esp32s31/system_internal.c:61-100`.
fn contain_secondaries<U: Placement, V: Placement, W: Placement, X: Placement, Y: Placement>(
    cores: &mut CoreControlRegisters<U, V, W, X, Y>,
) {
    for hart in crate::platform::Hart::SECONDARIES {
        cores::park(cores, hart);
        cores.set_boot_address(hart, 0);
    }
}

/// Open the APM access-path filters of every master: each enable written
/// zero, the other bits kept.
///
/// SOURCE: ESP-IDF 4d59230d `hp_apm_reg.h:1947-2013`, `lp_apm_reg.h:987-1026`,
/// `hp_mem_apm_reg.h:1211-1269` (`FUNC_CTRL`: M0..M6, M0..M3, M0..M5,
/// read-write, reset one); esp-hal 1f646c5 `esp-hal/src/soc/mod.rs:81-565`.
fn open_apm_filters<P: Placement, Q: Placement, R: Placement>(
    lp_apm: &mut LpApmFilter<P>,
    hp_apm: &mut HpApmFilter<Q>,
    hp_mem_apm: &mut HpMemApmFilter<R>,
) {
    lp_apm.modify_func_ctrl(|w| {
        w.set_m0_func_en(false)
            .set_m1_func_en(false)
            .set_m2_func_en(false)
            .set_m3_func_en(false);
    });
    hp_apm.modify_func_ctrl(|w| {
        w.set_m0_func_en(false)
            .set_m1_func_en(false)
            .set_m2_func_en(false)
            .set_m3_func_en(false)
            .set_m4_func_en(false)
            .set_m5_func_en(false)
            .set_m6_func_en(false);
    });
    hp_mem_apm.modify_func_ctrl(|w| {
        w.set_m0_func_en(false)
            .set_m1_func_en(false)
            .set_m2_func_en(false)
            .set_m3_func_en(false)
            .set_m4_func_en(false)
            .set_m5_func_en(false);
    });
}

/// Start the LP TRNG: its clocks open, a reset pulse, the noise source and
/// health-test cutoffs, the startup test's limit with the health tests on,
/// sampling and conditioning on, then the startup test started. Nothing
/// waits for the startup test: the entropy owner reads its status.
///
/// SOURCE: ESP-IDF 4d59230d `components/hal/esp32s31/include/hal/rng_ll.h:163-178`
/// (`rng_ll_enable`), esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/trng.rs`.
fn start_trng<S: Placement, T: Placement>(
    trng: &mut TrngInit<S>,
    clock: &mut LpPericlkrstRngInit<T>,
) {
    clock.modify_rng_ctrl(|w| {
        w.set_lp_rng_clk_en(true);
    });
    trng.modify_date(|w| {
        w.set_clk_en(true);
    });
    clock.modify_rng_ctrl(|w| {
        w.set_lp_rng_rst_en(true);
    });
    clock.modify_rng_ctrl(|w| {
        w.set_lp_rng_rst_en(false);
    });
    trng.modify_conf(|w| {
        w.set_noise_source_sel(NOISE_SELECT)
            .set_noise_pos_sel(NOISE_SELECT)
            .set_repetition_value_c(REPETITION_CUTOFF)
            .set_adpative_value_c(ADAPTIVE_CUTOFF);
    });
    trng.modify_debug_conf(|w| {
        w.set_startup_test_limit(STARTUP_TEST_LIMIT)
            .set_health_test_bypass(false);
    });
    trng.modify_conf(|w| {
        w.set_random_output_mode(true)
            .set_noise_crc_en(true)
            .set_sample_enable(true);
    });
    trng.trigger_debug_conf_startup_test_start();
}
