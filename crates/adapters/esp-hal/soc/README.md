# ESP32-S31 SoC services

This adapter retains the `oer-adapters-esp-hal-soc` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services. Radio register
ownership remains in `crates/esp32s31/pac`.

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

- `cache/maintenance.rs`: validates the borrowed PSRAM range and delegates
  writeback to HAL before a DMA reader observes memory.
- `cache/performance.rs`: cache counter snapshots and the retained CACHE witness.
- `flash/mmu.rs`: flash MMU operations with the retained SPI0 witness.
- `reset.rs`: a diverging system-reset request through HAL. The caller
  owns the failure policy and retains any live resources; this
  mechanism has no radio dependency and does not arm a watchdog or establish
  a reset-latency bound.
- `interrupt_table.rs`: the ESP32-S31 interrupt matrix the image's interrupt
  table (`oer-exec-interrupt-table`) drives. The platform runtime `adopt`s the
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
- `dma/mem2mem/descriptor.rs`: descriptor images, burst sizing, chain construction,
  and descriptor validation; its host tests live in `descriptor/tests.rs`.
- `dma/mem2mem/registers.rs`: typed upstream AXI-GDMA register operations,
  interrupt source controls, and DMA visibility fences.
- `dma/mem2mem/transfer.rs`: the DMA channel witness and prepared/active transfer
  owners retaining exclusive payload and descriptor borrows, including cleanup.
- `dma/mem2mem/completion.rs`: the channel-zero interrupt handler, static waker,
  and Future polling with its completion recheck.

The transfer owner binds both channel interrupts when constructed. Register
operations and completion share only the private mem2mem module boundary;
callers use the public root types. Preparation is safe; `PreparedOwner::start`
is unsafe because asynchronous publication borrows its payloads and descriptors.
The caller must keep them stable and exclusively retained until completion or
cleanup, including cancellation and forgotten futures. `Drop` alone cannot
prove that contract. A diagnostic caller must use dedicated static storage
and consume the sole channel; it must complete or drop each transfer before
reuse.
This borrowed diagnostic API is not the safe owned-buffer Wi-Fi materializer.

`axi-gdma-mem2mem` enables the hardware transfer path and implies `esp32s31`.
`psram-dma-diagnostic` additionally enables the existing blocking comparison
path. The descriptor sizing tests run on the host without those features.

HAL owns the cache sync engine and serializes writeback with its DMA and
executable-PSRAM cache operations. The adapter does not maintain a second
register sequence or lock for that engine.

CACHE maintenance and performance counters have distinct contracts:
writeback takes the affected mutable memory range, while performance
counters retain the CACHE witness. There is no shared CACHE lease or
coordinator between those APIs; the adapter does not establish that simultaneous use is safe.

Focused checks from the repository root:

```console
cargo test --locked --offline -p oer-adapters-esp-hal-soc
cargo check --locked --offline -p oer-adapters-esp-hal-soc --target riscv32imafc-unknown-none-elf --features axi-gdma-mem2mem
cargo check --locked --offline -p oer-adapters-esp-hal-soc --target riscv32imafc-unknown-none-elf --features psram-dma-diagnostic
```
