# ESP32-S31 SoC services

This adapter retains the `oer-espressif-esp-hal-soc` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services, and transfers
platform domains to the chip PAC. Radio register
ownership remains in `chips/esp32s31/pac`.

The private source modules follow the resources and operations they own:

- `cache/performance.rs`: cache counter snapshots and the retained CACHE witness.
- `flash/mmu.rs`: flash MMU operations with the retained SPI0 witness.
- `handoff.rs`: the per-domain transfers of platform blocks to the chip PAC.
  `take_efuse`, `take_entropy` and `take_reset_causes` consume the domain's
  esp-hal singletons (`RNG` and `LP_PERI`; `LP_AON_CLK_RST`; esp-hal hands
  out no `EFUSE`, so the eFuse transfer succeeds once by a flag) and
  return the PAC owner of its typed blocks, which the HAL's `platform`
  module reads (`FactoryIdentity`, `Entropy`, `ResetCauses`). esp-hal still
  enables the TRNG inside `esp_hal::init`; the entropy owner only reads it.
  `take_watchdogs` consumes `TIMG0`, `TIMG1` and `LP_WDT` and returns the
  watchdog blocks, which the HAL's `watchdog::Watchdogs` owns; timer group
  1's main watchdog serves the system deadline
  (`oer-espressif-watchdog::deadline`). esp-hal still stops every watchdog
  inside `esp_hal::init`.
  `take_usb_serial_jtag` retires the HAL's early USB writer if no early
  record holds the block (otherwise it returns `USB_DEVICE` with the
  reason), consumes `USB_DEVICE` and returns the console's owner, from which
  the HAL's `usb_serial_jtag::console_parts` gives the endpoint, the
  interrupt and the pads.

Performance counters retain the CACHE witness. Cache sync and writeback stay
with HAL, which serializes them with its DMA and executable-PSRAM cache
operations.

Focused checks from the repository root:

```console
cargo test --locked --offline -p oer-espressif-esp-hal-soc
cargo check --locked --offline -p oer-espressif-esp-hal-soc --target riscv32imafc-unknown-none-elf --features esp32s31
```
