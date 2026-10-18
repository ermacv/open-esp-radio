//! The Wi-Fi power-domain low-power clock selection of the pinned ESP-IDF
//! modem clock driver (`libesp_hw_support.a` over `libhal.a`), compared with
//! production `RadioPhyRegisters::select_wifi_low_power_clock` on the
//! `wifi-mac` leaf machinery. ESP-IDF's `esp_perip_clk_init` runs
//! `modem_clock_select_lp_clock_source(PERIPH_WIFI_MODULE, source, 0)` once at
//! start, with the 32 kHz crystal when the RTC slow clock runs from it and
//! the RC slow oscillator otherwise.
use crate::harness::Result;
use crate::mac::{Domain, Leaf, Objects, Vendor, in_archive, leaf, objects, quiet, ruled};

/// Session inputs of the third and fourth `wifi-mac` suite archives.
pub const HW_SUPPORT_INPUT: u64 = 5;
/// ESP-IDF `PERIPH_WIFI_MODULE` of `soc/esp32s31/include/soc/periph_defs.h`.
const PERIPH_WIFI_MODULE: u32 = 5;
/// ESP-IDF `MODEM_CLOCK_LPCLK_SRC_RC_SLOW` and `MODEM_CLOCK_LPCLK_SRC_XTAL32K`
/// of `hal/modem_clock_types.h`, which the production probe takes as is.
const MODEM_CLOCK_LPCLK_SRC_RC_SLOW: u32 = 0;
const MODEM_CLOCK_LPCLK_SRC_XTAL32K: u32 = 4;
const SOURCES: &[u32] = &[MODEM_CLOCK_LPCLK_SRC_RC_SLOW, MODEM_CLOCK_LPCLK_SRC_XTAL32K];
/// The divider `esp_perip_clk_init` passes.
const DIVIDER: u32 = 0;

/// `modem_clock_select_lp_clock_source(PERIPH_WIFI_MODULE, source, 0)`.
fn select_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: vec![PERIPH_WIFI_MODULE, words[0], DIVIDER],
        image: vec![crate::ble::modem_clock_hal_context(vendor)?],
        ..Default::default()
    })
}

/// The selection of each source `esp_perip_clk_init` chooses. The FreeRTOS
/// critical section and the sleep power-domain bookkeeping around the modem
/// clock registers are answered without effect.
pub const SELECT: Leaf = ruled(
    quiet(
        in_archive(
            objects(
                leaf(
                    "modem_clock_select_lp_clock_source",
                    "open_modem_clock_trace_select_wifi_low_power_clock",
                    &[("source", Domain::Words(SOURCES))],
                    false,
                ),
                select_abi,
            ),
            HW_SUPPORT_INPUT,
        ),
        crate::ble::MODEM_CLOCK_QUIET,
    ),
    wifi_power_gate_rules,
);

/// The Wi-Fi selection sets the Wi-Fi power gate.
fn wifi_power_gate_rules() -> Vec<blobray_domain::EffectRule> {
    crate::ble::platform_gate_rules(
        "esp_hal::init sets the Wi-Fi power gate once, as ESP-IDF's esp_perip_clk_init \
        does, and nothing clears it, so production leaves MODEM_LPCON.CLK_CONF to the platform",
    )
}
