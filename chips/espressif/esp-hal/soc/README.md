# ESP32-S31 SoC services

This adapter retains the `oer-espressif-esp-hal-soc` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services, and transfers
platform domains to the chip PAC. Radio register
ownership remains in `chips/esp32s31/pac`.

The private source modules follow the resources and operations they own:

`watchdog` is the public, non-radio TIMG1 deadline service. The board passes
its peripheral singleton once and moves the service into its task by value.
Each explicit nonzero `DeadlineBudget` moves the complete service into a
single non-cloneable lease. Timely completion disarms and returns the service
for the next obligation; cancellation, forgotten leases and late completion
never disarm or renew it. There is no task/interrupt mutex, no
periodic feed or timeout default. The pinned HAL uses XTAL; sleep and clock
changes while armed are unsupported. Reserve TIMG1 exclusively, including
independently constructible HAL Wdt handles. Expiry needs no task or ISR.
These are engineering limits, not measured worst-case reset/RF-off bounds.

- `cache/performance.rs`: cache counter snapshots and the retained CACHE witness.
- `flash/mmu.rs`: flash MMU operations with the retained SPI0 witness.
- `handoff.rs`: the per-domain transfers of platform blocks to the chip PAC.
  `take_efuse`, `take_entropy` and `take_reset_causes` consume the domain's
  esp-hal singletons (`RNG` and `LP_PERI`; `LP_AON_CLK_RST`; esp-hal hands
  out no `EFUSE`, so the eFuse transfer succeeds once by a flag) and
  return the PAC owner of its typed blocks, which the HAL's `platform`
  module reads (`FactoryIdentity`, `Entropy`, `ResetCauses`). esp-hal still
  enables the TRNG inside `esp_hal::init`; the entropy owner only reads it.
- `oer-espressif-esp-hal-irq`: the ESP32-S31 interrupt matrix the image's interrupt
  table (`oer-interrupt-table`) drives. The platform runtime's handoff adopts the
  image's table, silences each hart's sources when it installs the hart's
  interrupt stack and verifies them before it enables interrupts; an owner
  routes its source with `enable` and its token and silences it with
  `disable`. Compile-time assertions keep the table's 12-byte entry layout.

Performance counters retain the CACHE witness. Cache sync and writeback stay
with HAL, which serializes them with its DMA and executable-PSRAM cache
operations.

Focused checks from the repository root:

```console
cargo test --locked --offline -p oer-espressif-esp-hal-soc
cargo check --locked --offline -p oer-espressif-esp-hal-soc --target riscv32imafc-unknown-none-elf --features esp32s31
```
