# S31 RX chain hardware observation

This bootable probe makes one finite RX publication: 32 nodes of 128 bytes,
plus the production reserved descriptor R. It passively receives on channel 13
under the DE domain, stopping at 3 terminal nodes or 700 milliseconds. A 512-row
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
  cargo fw run ../probes/esp32s31-rx-chain --board s31-c --timeout 8s
```

The staged path builds through `cargo fw` as well. Build before leasing when
possible, then use the existing flash/monitor commands. This is a short fact
experiment, not an HIL scenario or a qualification image. No frame is transmitted.
The channel must match the laboratory AP before running.

`E` rows are hexadecimal words: node index (hex `20` = R, decimal 32), cycle before control,
cycle after control, control before/after the leading-word read, leading word,
BUFFER, descriptor NEXT, MAC BASE, MAC LAST/NEXT before the node sample,
MAC LAST/NEXT after it, unknown NEXT-upper nonzero before/after, and scan number.
Each row is a sequence of observations, not an atomic snapshot. `D` rows copy
all 32 buffer words only for nodes whose leading word differs from the sentinel
after the outer MAC and walker stop. Both E and D node indices are hexadecimal. Dumps may
contain received air data and belong in local evidence, never git.

The final state is dumped without rearm, then the production release closes RF.
Sampling gaps and scan bounds are reported. No absence or universal ordering
claim follows from an event that the sampling loop did not see.

`Q` rows report semantic STATUS and classified ENABLE observations. ENABLE tags
are software values: 0 = Disabled (every enable field read zero), 1 = Station,
2 = Unknown (outside those two reviewed images). An RX-only mask is therefore
2; this tag does not expose an individual enable bit image. Initial and final
masked zero STATUS samples alone prove no pending retention.

Version 3 enables only RX delivery 5/14/24 during the receive window, samples
statistics/semantic STATUS around each scan and acknowledges the sampled STATUS.
It does not reproduce the version-2 pending-replay experiment. `rx` is
RX_SUCCESS; `aux` combines 5/24. RAW and the individual auxiliary/opaque lanes
remain unavailable through the ordinary public HAL diagnostic API. No source120
CPU route is installed, so IRQ-entry/DMA ordering is not measured.

`C` rows contain 44 hexadecimal software words: scan, changed-leading bitmap
for ring nodes 0–31, reserved-leading-changed flag, terminal count, then two
20-word diagnostics bracketing that scan. Each diagnostic is: cycle before,
cycle after, ENABLE tag, semantic STATUS tag, then MPDU, END, FCS-error, abort,
abort-FCS-pass, power-drop, same-BM-error, other-unicast, last-unmatched,
buffer-full, FIFO-overflow, NRX-abort, NRX-restart, HT-SIG-error,
HE-SIG-A-CRC-error, data-success counters. The software STATUS tag uses bits
0..5 for TX-complete, RX-success, TX-timeout, color, auxiliary, unhandled; it is
not a register image. Counter snapshots are multi-register observations, not
atomic event identities. At most 192 correlation rows are retained. Both IRQ
sampling and counter reads can perturb timing, so correlation cannot alone
identify a rejected/aborted producer or prove rollback.

The descriptor alone does not mark nonterminal consumption. The repeated dirty
writes also leave the sentinel's validity as a consumption predicate unproved.

The capture stops at the first frame limit or deadline. If trace capacity is
exhausted, collection stops, then emits the retained rows and stopped-buffer
dumps before returning a failed verdict. The local bootstrap/cache and radio
startup fixes must be composed separately when the probe's base lacks them.

The probe retains exception/panic details in reset-retained memory through the
runtime's public hooks; a subsequent boot reports them before opening the live
console and stops with a failed verdict. Phase 1 precedes task setup, phase 2
covers public radio startup, and phase 3 follows its return. No handler takes
hardware from the console's live owner. These records diagnose pre-RX failure;
they do not observe MAC DMA completion.

The staged product-style stack check reports an unresolved P2 stack path.
This is a diagnostic, unqualified image; no panic budget or memory profile is
raised to make a product check pass.

The single three-beacon capture `hw18-20261027-rx-radio-01-s31b` used the
unpushed local composition `1f530932` (#687 + #700 and fallible-root handling),
not the PR head. It also shows 13 clusters of repeated leading-word changes
starting at ring nodes 0/3/6, with CONTROL still `0x80200080`, no RX_DONE,
original descriptor NEXT and stationary sampled MAC BASE/LAST/NEXT. Some clusters
rewrite nodes 3/4/6/7 before their later beacon contents; 24 ring nodes outside
the three completed chains remain dirty. Thus a non-sentinel leading word is
not established as membership in an accepted frame. Aborted/filtered reception
with rollback is a hypothesis requiring counter/event correlation. The external
report analyzes every dependent ring decision; this probe does not exercise the
production take/admission path or demonstrate a production ownership violation.
