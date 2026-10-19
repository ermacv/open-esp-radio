# ESP32-S31 SoC services

This adapter retains the `oer-espressif-esp-hal-soc` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services. Radio register
ownership remains in `chips/esp32s31/pac`.

The private source modules follow the resources and operations they own:

`watchdog` is the public, non-radio TIMG1 deadline service. The board passes
its peripheral singleton once and retains the service in stable storage.
Each explicit nonzero `DeadlineBudget` arms a single non-cloneable lease.
Completion before the deadline disarms; cancellation, forgotten leases,
stale completion and late completion never disarm or renew it. There is no
periodic feed or timeout default. The pinned HAL uses XTAL; sleep and clock
changes while armed are unsupported. Reserve TIMG1 exclusively, including
independently constructible HAL Wdt handles. Expiry needs no task or ISR.
These are engineering limits, not measured worst-case reset/RF-off bounds.

- `cache/performance.rs`: cache counter snapshots and the retained CACHE witness.
- `flash/mmu.rs`: flash MMU operations with the retained SPI0 witness.
- `reset.rs`: a diverging system-reset request through HAL. The caller
  owns the failure policy and retains any live resources; this
  mechanism has no radio dependency and does not arm a watchdog or establish
  a reset-latency bound.
- `interrupt_table.rs`: the ESP32-S31 interrupt matrix the image's interrupt
  table (`oer-interrupt-table`) drives. The platform runtime's handoff adopts the
  image's table, silences each hart's sources when it installs the hart's
  interrupt stack and verifies them before it enables interrupts; an owner
  routes its source with `enable` and its token and silences it with
  `disable`. Compile-time assertions keep the table's 12-byte entry layout.
- `entropy.rs`: owns RNG and the independent LP TRNG source. Fixed-size reads
  borrow that owner and release their temporary HAL reader before returning.
  No radio role or PHY epoch supplies its lifetime. Read pacing assumes a
  running CPU cycle counter; this is not a fault-time bound or an entropy
  quality assessment.
  `source_status` reads whether the TRNG's LP clock runs out of reset, sampling in the standard
  output mode and health-tested without a recorded failure. esp-hal enables
  it at startup with ESP-IDF's `rng_ll_enable` and keeps it running after a
  `TrngSource` drops, because `Rng` (MAC sequence numbers, backoff) reads the
  same generator.

Performance counters retain the CACHE witness. Cache sync and writeback stay
with HAL, which serializes them with its DMA and executable-PSRAM cache
operations.

Focused checks from the repository root:

```console
cargo test --locked --offline -p oer-espressif-esp-hal-soc
cargo check --locked --offline -p oer-espressif-esp-hal-soc --target riscv32imafc-unknown-none-elf --features esp32s31
```
