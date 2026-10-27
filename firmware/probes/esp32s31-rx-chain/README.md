# S31 RX chain hardware observation

This bootable probe makes one finite RX publication: 32 nodes of 128 bytes,
plus the production reserved descriptor R. It passively receives on channel 13
under the DE domain, stopping at 3 terminal nodes or 700 milliseconds. A 512-row
buffer records changes with mcycle bounds; no console output occurs during
collection. Nothing is taken, recycled, detached, reloaded, or repainted live.
Before BASE publication, every buffer is initialized word by word. Ordinary
leading words hold this probe's own `0xdeadbeef` pattern; other words equal
`0xa5000000 | (node << 12) | word_index`. Nodes 12, 20 and 28 use that pattern
for all 32 words, including the first. After the public start validates the
armed topology, their typed OWNER fields are cleared, with outer MAC RX still
stopped. No descriptor is rearmed during the capture. Walker prefetch before
this live OWNER change is not excluded; it is relevant to the detach premise.

All hardware ownership and MMIO go through the public production API. The
probe alone also reads DMA buffer words with volatile loads through the
published BUFFER address: it checks the original production binding, alignment,
capacity and containing permanent allocation and creates no byte references.
The probe initializes bytes only before BASE publication, never during RX. This narrow observation detects a first-word change independently
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
all 32 buffer words of every node after the outer MAC and walker stop. Both E and D node indices are hexadecimal. Dumps may
contain received air data and belong in local evidence, never git.

The final state is dumped without rearm, then the production release closes RF.
Sampling gaps and scan bounds are reported. No absence or universal ordering
claim follows from an event that the sampling loop did not see.

`Q` rows report semantic STATUS and classified ENABLE observations. ENABLE tags
are software values: 0 = Disabled (every enable field read zero), 1 = Station,
2 = Unknown (outside those two reviewed images). An RX-only mask is therefore
2; this tag does not expose an individual enable bit image. Initial and final
masked zero STATUS samples alone prove no pending retention.

The capture enables only RX delivery 5/14/24 during the receive window, samples
statistics/semantic STATUS around each scan and acknowledges the sampled STATUS. `rx` is
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

SOURCE[`HW18_RX_HEAD_LEADING_PRESERVATION_20261027`] records 23 observed
uncommitted-write clusters: 13 with the production sentinel and 10 with the
CPU barriers. Both finite captures use one s31-b rev0 board, channel 13,
32×128-byte ring nodes plus R, and three accepted beacons with a
three-terminal/700-ms bound, without take/recycle/rearm. The barrier capture
has FCS=9 and buffer-full=8, with its first OWNER=0 node at index 12.
No selected cluster changes the leading word of the current sampled NEXT
head: changes start at its linked successor, node 0 after R, node 3 after
NEXT=2, or node 6 after NEXT=5. These are clusters, not independently identified
peer frames, and whole-head preservation is unmeasured. A head can already
be dirty from an earlier series before becoming NEXT.
The result narrows the observed rejected-write footprint; it neither validates
sentinel accepted-frame membership nor proves arbitrary in-flight detach/recycle safety.

The capture stops at the first frame limit or deadline. If trace capacity is
exhausted, collection stops, then emits the retained rows and stopped-buffer
dumps before returning a failed verdict. A pass means bounded collection
completed, even if the barriers prevented every terminal; air coverage and the
hardware conclusion are assessed separately. The probe builds directly on main.

The probe retains exception/panic details in reset-retained memory through the
runtime's public hooks; a subsequent boot reports them before opening the live
console and stops with a failed verdict. Phase 1 precedes task setup, phase 2
covers public radio startup, and phase 3 follows its return. No handler takes
hardware from the console's live owner. These records diagnose pre-RX failure;
they do not observe MAC DMA completion.

The staged product-style stack check reports an unresolved P2 stack path.
This is a diagnostic, unqualified image; no panic budget or memory profile is
raised to make a product check pass.

`G` rows compare every word of each CPU-owned buffer on every scan, and
retain a row when its mismatch bitmap changes or a full scan is forced. The
ten hex words are scan, node, cycle before/after, CONTROL before/after,
32-bit differing-word bitmap, BASE, LAST and NEXT. At most 64 rows are kept.
A zero final bitmap is only a sampled/final-state observation, not proof that
a transient write followed by restoration never happened.

At each new terminal count, a bounded `K`/`F` snapshot copies all nodes. `K`
contains snapshot ordinal then scan, terminal count, cycle before/after, BASE,
LAST/NEXT before and LAST/NEXT after. `F` contains ordinal, node, CONTROL
before, all 32 buffer words, CONTROL after. At most three snapshots are retained.
These copies preserve accepted-frame bytes before later attempts can overwrite
them, but their reads and cursor samples are sequential, not atomic. Missed
intermediate cursor states and multiple completions within one scan remain
possible. All buffers stay retained even if OWNER=0 fails to block DMA.
