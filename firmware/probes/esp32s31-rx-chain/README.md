# S31 RX chain hardware observation

This bootable probe makes one finite RX publication: 96 nodes of 256 bytes,
plus the production reserved descriptor R. It passively receives on channel 13
under the DE domain, stopping at 32 terminal nodes or 4.5 seconds. A 2,048-row
buffer records changes with mcycle bounds; no console output occurs during
collection. Nothing is taken, recycled, detached, reloaded, or repainted live.
The production preparation path installs the leading `0xdeadbeef` word.

All hardware ownership and MMIO go through the public production API. The
probe alone also reads DMA buffer words with volatile loads through the
published BUFFER address: it checks the original production binding, alignment,
capacity and containing permanent allocation, creates no references, and writes
no buffer bytes. This narrow observation detects a first-word change independently
of descriptor completion. It is neither a completion nor ownership proof.

Build with `cargo fw probe oer-firmware-probes-esp32s31-rx-chain`. The ELF is a
staged runtime, requiring the normal bootstrap/PSRAM handoff. To package and run
that same source through the existing staged pipeline, under a board/air lease:

```console
cargo stand --owner wifi lease --board s31-c --air shared -- \
  cargo fw run ../probes/esp32s31-rx-chain --board s31-c --timeout 30s
```

The staged path builds through `cargo fw` as well. Build before leasing when
possible, then use the existing flash/monitor commands. This is a short fact
experiment, not an HIL scenario or a qualification image. No frame is transmitted.
The channel must match the laboratory AP before running.

`E` rows are hexadecimal words: node index (96 = R), cycle before control,
cycle after control, control before/after the leading-word read, leading word,
BUFFER, descriptor NEXT, MAC BASE, MAC LAST/NEXT before the node sample,
MAC LAST/NEXT after it, unknown NEXT-upper nonzero before/after, and scan number.
Each row is a sequence of observations, not an atomic snapshot. `D` rows copy
all 64 buffer words after the outer MAC and walker have stopped. Dumps may
contain received air data and belong in local evidence, never git.

The final state is dumped without rearm, then the production release closes RF.
Sampling gaps and scan bounds are reported. No absence or universal ordering
claim follows from an event that the sampling loop did not see.

The RX experiment currently stops in the public radio-start path after stage-two
entry. With the bootstrap PSRAM/cache-placement fixes applied locally, s31-c
reaches `RXCHAIN boot` and `RXCHAIN radio-start` but produces no `RXCHAIN begin`
or descriptor records in the bounded capture (`hw4-20261026-rx-chain-08-s31c`).
No sentinel-replacement or RX timing fact follows from this launch.
The bootstrap fixes are separate from this probe's branch. See the external
report `/home/ermacv/dev/oer-notes/research/hw-rx-chain-descriptors.md`.

The probe retains exception/panic details in reset-retained memory through the
runtime's public hooks; a subsequent boot reports them before opening the live
console and stops with a failed verdict. Phase 1 precedes task setup, phase 2
covers public radio startup, and phase 3 follows its return. No handler takes
hardware from the console's live owner. These records diagnose pre-RX failure;
they do not observe MAC DMA completion.

The staged product-style stack check reports an unresolved P2 stack path.
This is a diagnostic, unqualified image; no panic budget or memory profile is
raised to make a product check pass.
