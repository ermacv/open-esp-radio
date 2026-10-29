//! S31 profiles and cold-start platform binding, separate from HAL atoms.
#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
use oer_esp32s31_hal::boot::{Bits, LpFastSource};
use oer_espressif_boot::{
    drivers::{brownout, clock, entropy},
    wait::BootWait,
};
use oer_time::Duration;

include!(concat!(env!("OUT_DIR"), "/lp-fast-source.rs"));

/// SOURCE: ESP-IDF 4d59230d `rtc_clk.c:122-126`, `soc/rtc.h:56`:
/// `SOC_DELAY_RTC_FAST_CLK_SWITCH`, 3 µs after the selector write.
pub const LP_FAST: clock::LpFastProfile<LpFastSource> = clock::LpFastProfile {
    source: LP_FAST_SOURCE,
    settle: const {
        match BootWait::new("lp-fast-switch", Duration::from_micros(3)) {
            Ok(wait) => wait,
            Err(_) => panic!("nonzero LP-fast settle"),
        }
    },
};

/// SOURCE: ESP-IDF 4d59230d `brownout.c`, `brownout_hal.c`,
/// `esp32s31/brownout_ll.h`, `esp32s31/Kconfig.power`: mode 0 hardware
/// system reset, flash/RF power-down, waits 2/0x3ff, threshold level 7.
/// The 1 ms bound replaces the vendor's unbounded `regi2c_write_mask`.
pub const BROWNOUT: brownout::Profile<Bits<10>> = brownout::Profile {
    settings: brownout::Settings {
        interrupt_wait: Bits::constant::<2>(),
        reset_wait: Bits::constant::<0x3ff>(),
        close_flash: true,
        power_down_rf: true,
        reset_enabled: true,
        system_reset: true,
    },
    threshold: 7,
    threshold_wait: const {
        match BootWait::new("bod-threshold", Duration::from_millis(1)) {
            Ok(wait) => wait,
            Err(_) => panic!("nonzero threshold bound"),
        }
    },
};

/// SOURCE: ESP-IDF 4d80c1b2 `hal/esp32s31/include/hal/rng_ll.h:163-178`:
/// noise source/position 0x10, repetition/adaptive cutoffs 0x1f/0x12,
/// 1024 startup health-test samples.
pub const ENTROPY: entropy::Profile<Bits<5>, Bits<9>> = entropy::Profile {
    noise_source: Bits::constant::<0x10>(),
    noise_position: Bits::constant::<0x10>(),
    repetition_cutoff: Bits::constant::<0x1f>(),
    adaptive_cutoff: Bits::constant::<0x12>(),
    startup_samples: 1024,
};

const _: () = {
    if !oer_esp32s31_hal::boot::BOOT_BUDGET.admits(LP_FAST.settle)
        || !oer_esp32s31_hal::boot::BOOT_BUDGET.admits(BROWNOUT.threshold_wait)
    {
        panic!("the unchanged L3 waits fit the unchanged boot budget");
    }
};

/// Lend the detector and its analog bus together, using the chip profile.
pub fn apply_brownout<C: oer_time::Clock + ?Sized>(
    owner: &mut oer_esp32s31_hal::boot::Brownout,
    deadline: &oer_espressif_boot::wait::BootDeadline<'_, C>,
) -> Result<(), brownout::BrownoutFailure> {
    let (registers, mut bus) = owner.parts();
    brownout::apply(registers, &mut bus, BROWNOUT, deadline)
}

#[cfg(target_arch = "riscv32")]
mod startup;
#[cfg(target_arch = "riscv32")]
pub use startup::S31;
