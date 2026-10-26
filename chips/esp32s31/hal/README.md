# ESP32-S31 HAL

Register logic over the thin PAC (docs/architecture.md, "Layers"): the
reset root, sequences, polls, routing, clocks, power, resets and DMA
engines.

The reset root's `into_bootstrap` and `into_runtime` return `Result` with
`root::InitError`: a refused console installation, a stuck bootstrap
monotonic latch, or a refused stage-two core-control installation is a
named boot failure. Image callers handle it without a panic or fallback.

The watchdog owners retain both timer groups' clock/reset words. Runtime
startup selects XTAL and enables TIMG0 T0 through a private watchdog operation
that changes only `T0_SRC_SEL/T0_CLK_EN`; it preserves group reset, APB, T1 and
watchdog clocks. MWDT0 keeps that authority inside `StoppedMainWatchdog`.
The separate T0 counter and interrupt-bank parts have no group-reset access;
the HAL exposes no T0 timer API.

## Platform clocks

`power::ChipClocks` is the one owner of the platform clock words the radio
shares with the SoC (`MODEM_LPCON.CLK_CONF`, `HP_SYS_CLKRST.REF_160M_CTRL0`):
`Root::into_runtime` installs it as `Runtime::clocks`, it counts the
references of every user, and a refused acquire or release is the caller's
typed error.

Installation consumes the unique `ChipClockPeripherals` from the root by
value. These fixed-placement PAC owners cannot be cloned or recovered after
installation, so a second installation cannot be expressed. The paired UI
case `chip_clocks_installed_once` checks that consuming `Root` produces the
clock owner and prevents installing it again with that root.

- MPLL: the PSRAM mapping holds it for the whole image (`PsramMpll`), so a
  radio reference is a count and no path powers MPLL down (ESP-IDF 4d59230d
  `esp_clk_tree.c` `esp_clk_tree_mpll_acquire`; esp-hal 1f646c5
  `soc/esp32s31/clocks.rs:265-321`, PSRAM's never-released reference).
- BBPLL, the 160 MHz output's parent, stays powered for the whole image
  (the flash clock and the USB Serial/JTAG console run on it; ESP-IDF
  4d59230d `esp_clk_tree.c:404-408`), so the 160 MHz edge is its gate only.
- The analog-I2C master's reset is not pulsed: no ESP32-S31 ESP-IDF path
  calls `regi2c_ctrl_ll_master_reset` (ESP-IDF 4d59230d
  `esp_hal_regi2c/esp32s31/include/hal/regi2c_ctrl_ll.h:43-53`).
- The Wi-Fi power gate opens once, at installation, and nothing closes it
  (esp-hal 1f646c5 `rtc_cntl/rtc/esp32s31.rs:670`, `enable_wifi_power_clock`).

## Bluetooth register transactions

The interrupt bank clears and enables baseline images `0x8000` / `0x1300`,
then masks the same sources on release while preserving unrelated enables.
The current vendor pin includes bank-one source 1 in baseline `0x1302`;
its cause identity is unknown and the HAL does not arm it until B1 identifies
the event and its handling. The modem LP timer
samples the low bytes of its pending and rollover words, acknowledges each
nonzero byte with a reviewed whole-word zero, and publishes software pending
with a whole-word one. These stores include the unmodeled upper bytes.
SOURCE: `BLOB_LIBBTDM_CONTROLLER_IRQ` (HAL baseline),
`BLOB_LIBBTDM_BASELINE_IRQ_CURRENT` (N5, current vendor baseline) and
`BLOB_LIBBTDM_LP_PENDING_WORDS_CURRENT` (L4/L6/L7) in the register evidence.

The IRQ control commands, LP command lanes and mode controls have unknown
internal actions. The NRT snapshots do not establish an output-mask equation,
and compare disable does not prove all-cause quiescence. The register model
records those limits; the transactions supply no additional output-off proof.

Modem PLL source gating uses the pinned ESP-IDF 4d59230d whole-word sequence:
open `0x15 → 0x1d`, close `0x2d → 0x25`. Platform source-clock readback
requires the open PLL image with the XTAL gate clear
(`ESP_IDF_4D59230D_S31_MODEM_PLL_GATE`).

Ordinary legacy, HT and HE queue preparation sets CONTROL bit 22. The
reserved-TXOP-slot adjustment is a known vendor difference, documented in
[the driver](../ieee80211/driver/README.md#tx-control-bit-22)
(`VENDOR_WIFI_TXOP_DESCRIPTOR_CLASS`).
FE `+0x8d0[21:20]` remains opaque: its nonzero sample contributes diagnostic
and RX-saturation observations, while estimator completion uses READY.

## Wi-Fi watchdog and interrupt evidence

Baseband initialization follows ROM `phy_bb_wdg_cfg`: it sets watchdog control
low16 to `0xaa` and control30, then enables `WIFI_BB+c40[31]`, the hardware
BB reset action on timeout. It preserves the independent interrupt-enable gate
at [30]; [29] is the timeout-clear request. The extent of the hardware BB reset
has not been measured. These fields do not establish a counter run gate or a
complete source123 interrupt/status/acknowledgement lifecycle.
SOURCE: `ROM_REV0_BB_WATCHDOG_LANES_20261026` in the register evidence.

Power source122 STATUS hides masked TSF timer0/1 events while RAW retains them;
a retained positive sample can clear them under the mask. A fresh zero STATUS
sample cannot clear hidden RAW. The masked clear-all transaction also clears
local timer0 pending. These stand facts do not qualify TBTT, beacon or opaque
lanes, nor simultaneous set/clear priority.
SOURCE: `HW8_S31_WIFI_TSF_MASK_20261026`.
