#![no_main]
#![no_std]

//! The ESP32-S31 bootstrap: the staged boot's chip-neutral steps
//! (`oer-espressif-staged-bootstrap`) around this board's PSRAM and its
//! 120-MHz Flash tuning, which needs the stage-two copy made first and a
//! cold XIP span.

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "oer-esp32s31-platform-bootstrap",
    "00:00:00",
    "2026-07-31",
    "6.1",
    oer_esp32s31_platform_board::FLASH_MMU_PAGE_SIZE_BYTES,
    0,
    u16::MAX,
    0
);

use core::mem::size_of;

use oer_esp32s31_platform_board as board;
use oer_esp32s31_platform_layout::LAYOUT;
use oer_esp32s31_soc_esp_hal::FlashMmu;
use oer_espressif_staged_bootstrap::{self as staged, fail, print};
use static_cell::ConstStaticCell;

const FLASH_TUNING_REFERENCE_WORDS: usize =
    LAYOUT.flash_tuning_reference_bytes as usize / size_of::<u32>();

// This is the load image, not its runtime placement. The bootstrap copies and
// verifies it at the bootloader's qualified 80 MHz setting before changing
// the Flash timing. The tuning sweep uses a separate, still-cold XIP span.
#[used]
#[unsafe(link_section = ".psram.runtime.payload")]
static RUNTIME_PAYLOAD: [u8; include_bytes!(env!("PSRAM_RUNTIME_BIN")).len()] =
    *include_bytes!(env!("PSRAM_RUNTIME_BIN"));

// Flash tuning must not sample pages already pulled into cache by the runtime
// copy. This private reference span is physically separate from the stage-two
// image and contains an incompressible deterministic pattern.
//
// SOURCE: pinned esp-hal fork, `esp-hal/src/flash.rs::Flash::tune_120mhz`,
// which requires at least fifteen distinct 4-KiB XIP pages.
#[used]
#[unsafe(link_section = ".flash.tuning.reference")]
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = flash_tuning_reference();

// Flash tuning compares complete XIP pages while the flash cache is disabled,
// so its 124-KiB scratch area must live in internal SRAM. Keeping this unique
// owner static prevents the early-boot stack from being consumed by a
// short-lived local array; tune_120mhz still validates every original page.
static FLASH_TUNING_SCRATCH: ConstStaticCell<[u32; esp_hal::flash::FLASH_TUNING_SCRATCH_WORDS]> =
    ConstStaticCell::new([0; esp_hal::flash::FLASH_TUNING_SCRATCH_WORDS]);

#[esp_hal::main]
fn main() -> ! {
    staged::start();

    let mut peripherals = esp_hal::init(esp_hal::Config::default());
    print(c"OER_BOOT bootstrap=INIT\r\n");
    apply_brownout_policy(&mut peripherals.I2C_ANA_MST);
    print(c"OER_BOOT bootstrap=BROWNOUT\r\n");
    let mut flash_mmu = FlashMmu::new(peripherals.SPI0);
    let mut flash =
        match esp_hal::flash::Flash::new(peripherals.FLASH, esp_hal::flash::Config::default()) {
            Ok(flash) => flash,
            Err(_) => fail(c"OER_BOOT bootstrap=FAIL reason=flash-init\r\n"),
        };

    let psram = board::initialize_psram(peripherals.PSRAM);
    let (psram_base, psram_size) = psram.raw_parts();
    // SAFETY: `initialize_psram` mapped and initialized the board's PSRAM
    // at `psram_base..psram_base + psram_size` (word aligned) for reads and
    // writes; `psram` stays alive and mapped until `hand_off` and the boot
    // uses that region nowhere else, so `stage` owns it exclusively.
    let staged = unsafe { staged::stage(&LAYOUT, &RUNTIME_PAYLOAD, psram_base, psram_size) };

    let tuning_address = FLASH_TUNING_REFERENCE.as_ptr() as usize;
    let tuning_physical_start = flash_mmu
        .physical_address(tuning_address)
        .unwrap_or_else(|| fail(c"OER_BOOT bootstrap=FAIL reason=tuning-physical\r\n"));
    // SAFETY: the reference span is this bootstrap's own cold Flash rodata,
    // read by nothing else; stage two was copied from a disjoint range first.
    if unsafe {
        flash.tune_120mhz(
            esp_hal::flash::FlashXipRegion {
                physical_start: tuning_physical_start,
                virtual_start: tuning_address,
                size: size_of::<[u32; FLASH_TUNING_REFERENCE_WORDS]>(),
            },
            FLASH_TUNING_SCRATCH.take(),
        )
    }
    .is_err()
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=flash-tune\r\n");
    }
    print(c"OER_BOOT bootstrap=FLASH_TUNED\r\n");
    // Do not read the tuning span again. Every candidate deliberately fetches
    // a distinct page, and rejected timings can leave corrupted cache lines
    // behind until a future S31 cache-invalidate primitive is available. The
    // region is disposable; stage two was copied from a disjoint range first.
    staged::hand_off(staged)
}

/// Replace the bootloader's analog mode-1 brownout reset with ESP-IDF's
/// application policy, once, before the application runs, and stop the boot
/// unless the detector reads back in that policy.
///
/// SOURCE: ESP-IDF 4d59230d `esp_brownout_init`
/// (`components/esp_hw_support/power_supply/brownout.c`), run at startup by
/// `init_brownout` (`components/esp_system/startup_funcs.c`). The ESP32-S31
/// has no `ESP_BROWNOUT_USE_INTR`, so it configures mode 0 with the hardware
/// reset enabled, flash and RF power-down, at `ESP_BROWNOUT_DET_LVL` 7
/// (about 2.4 V), the S31 default of
/// `components/esp_hw_support/power_supply/port/esp32s31/Kconfig.power`.
/// `BrownoutConfig::default()` is that configuration; `brownout::configure`
/// follows `brownout_hal_config`. The threshold is an analog register that
/// esp-hal writes but does not read back, so the check covers the detector's
/// mode, actions and waits.
fn apply_brownout_policy(analog_bus: &mut esp_hal::peripherals::I2C_ANA_MST<'_>) {
    use esp_hal::rtc_cntl::brownout::{self, BrownoutConfig};

    brownout::configure(analog_bus, BrownoutConfig::default());
    let lp_ana = esp_hal::peripherals::LP_ANA::regs();
    let mode0 = lp_ana.bod_mode0_cntl().read();
    let mode1 = lp_ana.bod_mode1_cntl().read();
    // A clear `BOD_MODE1_FIB` hands mode 1 to software
    // (`brownout_ll_ana_reset_enable`).
    let mode1_software = !lp_ana.fib_enable().read().bod_mode1_fib().bit();
    let configured = mode1_software
        && !mode1.bod_mode1_reset_ena().bit()
        && mode0.bod_mode0_reset_ena().bit()
        && mode0.bod_mode0_reset_sel().bit()
        && mode0.bod_mode0_intr_ena().bit()
        && mode0.bod_mode0_pd_rf_ena().bit()
        && mode0.bod_mode0_close_flash_ena().bit()
        && mode0.bod_mode0_reset_wait().bits() == 0x3ff
        && mode0.bod_mode0_intr_wait().bits() == 2;
    if !configured {
        fail(c"OER_BOOT bootstrap=FAIL reason=brownout-policy\r\n");
    }
}

const fn flash_tuning_reference() -> [u32; FLASH_TUNING_REFERENCE_WORDS] {
    let mut words = [0_u32; FLASH_TUNING_REFERENCE_WORDS];
    let mut state = 0x31a5_c33c_u32;
    let mut index = 0;
    while index < words.len() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        words[index] = state ^ (index as u32).rotate_left((index & 31) as u32);
        index += 1;
    }
    words
}
