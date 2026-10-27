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
`Runtime::timg0_alarm` adopts two disjoint owners without writes:
`Timg0AlarmProgram` owns T0 configuration, alarm, load and the whole ENA word;
`Timg0AlarmInterrupt` owns the whole RAW/ST words and only T0 CLR. Both lack
group-reset access. The program owner can travel in the actor's transaction
bundle: the actor can arm before depositing it, and the handler can rearm
while holding it.
Only the interrupt owner is bound permanently to the handler. They share no
software state or lock; each transition takes its own owner's `&mut`.

`arm_after` requires the IRQ owner's router `Bound` or `Silent` proof and a
positive relative microsecond interval within generated counter geometry.
It stops and masks T0, reloads zero, starts an increasing XTAL/divider40 one-shot
and unmasks delivery. It does not acknowledge an existing cause. `disarm`
only stops and masks; RAW remains for the handler's IRQ owner to observe and
acknowledge separately. The IRQ owner's `acknowledge` samples RAW/ST and
strobes only T0 CLR, leaving sibling causes, programming and ENA unchanged.
Zero and out-of-range input return `InvalidDelay` before MMIO. Separate
comparator observations at stopped count10000 cover equal/below targets;
zero-delay arming remains outside this API.

SOURCE for stop/load-zero/T0LOAD-strobe/start: ESP-IDF 4d59230d,
`components/esp_hal_timg/esp32s31/include/hal/timer_ll.h:236-239` (counter),
`:293-297` (reload value), `:319-322` (software reload).
`HW23_S31_TIMG0_RELOAD_20261027` confirms this sequence on S31 at
XTAL/divider40 for 10/100/1000 µs and with pending retained across rearm.
LOAD resets both counter halves; programming LOADHI/LO alone does not.
Instrumented RAW observations agree with nominal 1 MHz counting; they do
not measure exact comparator or interrupt-entry latency.
`HW23_S31_TIMG0_COMPARATOR_20261027` observes no expiry at/below the stopped
count until T_EN is set, then prompt expiry; a running past target also fires.
`HW7_S31_TIMG0_20261026` in the register evidence supports one-shot expiry,
RAW retention under stop/mask, ENA redelivery and CLR at ENA=0/1, without a
clear/arrival collision or exact IRQ-latency guarantee.

The host fixture selects `Exempt(ExecutorLevel)` at P2 with a lower line ID
than FROM_CPU2. SOURCE: `chips/espressif/platform/family.toml` [interrupts],
RISC-V CLIC specification 3f8ee744, "Specifying Interrupt Priority";
ESP-IDF 4d59230d `components/soc/esp32s31/include/soc/clic_reg.h` supplies the
CLIC geometry. SOURCE for priority/ID arbitration:
[ESP32-C5 TRM v1.1](https://www.espressif.com/sites/default/files/documentation/esp32-c5_technical_reference_manual_en.pdf),
section 2.8.2.5, p.86. `HW23_S31_P2_ARBITRATION_20261027` confirms line17
before16 on S31 hart0 at P2, with TIMG0/FROM_CPU2 assigned both ways and
both positively pending before admission. Other pairs, levels and sustained
fairness remain unmeasured. No production image binds or arms T0;
PHY uses the existing P2 executor.

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

The interrupt bank clears and enables baseline images `0x8000` / `0x1302`,
including `CONTROLLER_TIME_OVERFLOW` in bank one. A primary status snapshot
carries this normal event separately from fault and dynamic scheduler sources.
CONTROL0 starts BTLC. CONTROL1/2 do not lower an enabled pending cause, so
release masks and clears overflow before those commands and masks the remaining
baseline sources, preserving unrelated enables. Masking alone retains pending.
SOURCE: `BLOB_LIBBTDM_BASELINE_IRQ_CURRENT` (N5),
`HW9_S31_BTLC_START_20261027`, `HW9_S31_MAC_RELEASE_20261027`,
`HW9_S31_MAC_MASK_ACK_20261027` and
`HW12_S31_BLE_OVERFLOW_ACK_20261026` in the register evidence.
The measured overflow is visible in primary status/source 124 while the NRT
snapshots/source 133 remain zero; their zero acknowledgement does not consume
it. The exact sampled NRT pair is checked against the primary overflow clear
lane before either acknowledgement. If that lane is present, a typed fault
retains both words and neither bank is acknowledged. This does not identify
other NRT producers or simultaneous set/clear priority.

The modem LP timer samples the low bytes of its pending and rollover words,
acknowledges each nonzero byte with a reviewed whole-word zero, and publishes
software pending with a whole-word one. These stores include the unmodeled
upper bytes. SOURCE: `BLOB_LIBBTDM_LP_PENDING_WORDS_CURRENT` (L4/L6/L7).
LP command lanes and mode controls retain their measured scope; compare disable
does not prove all-cause quiescence.

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
