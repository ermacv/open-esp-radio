#![no_main]
#![no_std]

//! The ESP32-C5 bootstrap: the staged boot's chip-neutral steps
//! (`oer-espressif-staged-bootstrap`) around the module's PSRAM. The chip's
//! flash stays at the image header's 80 MHz, so no tuning follows the copy.
//!
//! The bootstrap enables no radio or modem clock. ESP-IDF's modem clock
//! initialization widens `MODEM_LPCON` `CLK_CONF_POWER_ST.clk_i2c_mst_st_map`
//! to the MODEM power state, which survives a USB Serial/JTAG reset and makes
//! this revision's ROM boot into UART/SDIO download (see
//! `docs/hardware-errata.md`); esp-hal's initialization leaves the field at
//! its reset value.

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "oer-esp32c5-platform-bootstrap",
    "00:00:00",
    "2026-10-05",
    "6.1",
    oer_esp32c5_platform_board::FLASH_MMU_PAGE_SIZE_BYTES,
    0,
    u16::MAX,
    0
);

use oer_esp32c5_platform_board as board;
use oer_esp32c5_platform_layout::LAYOUT;
use oer_espressif_staged_bootstrap::{self as staged, print};

// This is the load image, not its runtime placement: the bootstrap copies
// and verifies it into PSRAM.
#[used]
#[unsafe(link_section = ".psram.runtime.payload")]
static RUNTIME_PAYLOAD: [u8; include_bytes!(env!("PSRAM_RUNTIME_BIN")).len()] =
    *include_bytes!(env!("PSRAM_RUNTIME_BIN"));

#[esp_hal::main]
fn main() -> ! {
    staged::start();

    let peripherals = esp_hal::init(esp_hal::Config::default());
    print(c"OER_BOOT bootstrap=INIT\r\n");

    let psram = board::initialize_psram(peripherals.PSRAM);
    let (psram_base, psram_size) = psram.raw_parts();
    // SAFETY: `initialize_psram` mapped and initialized the board's PSRAM
    // at `psram_base..psram_base + psram_size` (word aligned) for reads and
    // writes; `psram` stays alive and mapped until `hand_off` and the boot
    // uses that region nowhere else, so `stage` owns it exclusively.
    let staged = unsafe { staged::stage(&LAYOUT, &RUNTIME_PAYLOAD, psram_base, psram_size) };
    staged::hand_off(staged)
}
