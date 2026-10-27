# S31 RX chain hardware observation

The default source selection is **200**, a source120 traffic/take/recycle observation.
The `legacy-observer` package feature selects the original actor composition
and selection160; its source constant can select the selection4 barrier observer.
All modes use the same public cold hardware transitions and one
retained 32×128-byte RX arena plus R. Only200 performs take/recycle/rearm.

## Traffic and ownership capture (selections200–209)

Selection200 runs one 700-ms maximum RX window, or stops at twelve taken
units. It uses the existing permanent 32×128-byte arena plus R. Every MMIO
operation borrows the public HAL owner. There is no source120 CPU handler;
timestamps describe peripheral visibility, not ISR entry. Nothing in production
calls the independent opaque-enable observation added for this measurement.

The new mode calls the existing backend's public `prepare`, `enter` and
`track` transitions directly on P2, in production order. Its placed futures
avoids the actor request/answer frames around registration, which exceed
the unchanged IRQ-stack contract at this base. A `ClaimCell` transfers only
the resulting permanent running owner's reference to P1. The backend, clocks
and running owner remain retained until the leased host reset; no reusable
radio is returned and no independent PAC claim is made. Only the selected
composition's P2 task roots are compiled. Modes4/160 retain their original
actor composition and its currently unqualified stack path.

Checkpoints200/201 bracket this public cold startup;202 follows DMA publication
and a sampled nonfatal stale RAW acknowledgement, before outer RX starts.
Phase203 publishes the reviewed station interrupt mask and records enabled
reception for350ms, then204 masks every MAC enable without acknowledgement
for the rest of the window. Both phases exercise real frozen-cursor
`take_completed_unit`, byte reads, `retain_discard` and affine recycle-credit
`append` through frozen LAST, immediately completing each
pending reload with the production20-µs settle. Recording errors retain the
unit for deferred recycling and stop collection. No live payload is painted.

Every scan copies all33 buffers by bounded volatile value reads **before**
RAW and the exact pre-ack STATUS sample. The existing observation checks use
the original pre-RX BUFFER bindings. The taken unit's complete segments are
copied and compared against those earlier bytes and the preceding scan's
retained byte copy, then read again through the completed view before recycling.
A reserved-descriptor continuation is explicitly marked as an incomplete frame;
it cannot qualify whole-frame payload ordering. A unit taken on a later scan
than first positive14 also cannot qualify that earlier observation. Finite
equal bytes constrain sampled order only; they do not prove electrical IRQ
assertion order, a universal DMA guarantee or arbitrary detach safety.

After stopping outer RX and the walker,205 records masked pending;206
enables RX5/14/24 and retains three independent STATUS samples.207 masks all
enables, records pre-clear state and clears only saved5;208 clears saved14;
209 clears saved24, then observes RX-unmask replay and final all-enable mask.
Each exact clear sample is emitted before consumption; zero samples remain
inconclusive. No masked STATUS resample replaces the saved authority.

`M` rows contain32 hex words: scan, phase, pre-copy begin/end cycle, RAW and
its end cycle, STATUS and its end cycle, LAST/NEXT, DONE bitmap and R DONE,
RX-enable software code (5=1/14=2/24=4), opaque-enable software code
(6=1/12=2/13=4/23=8), MPDU/END/FCS/buffer-full/data-success counters,
taken head+1/count/length/reserved-continuation/mismatching-byte count,
take-end cycle, append-success flag, post-ack RAW/STATUS/end cycle,
stopped substep, exact selected-clear sample (phase206 only), reserved zero.
Bank-only checkpoints have zero DMA/counter columns.
At most256 rows are retained, with16 reserved for stopped checkpoints.

`T` rows contain unit ID, then scan/phase/RAW/STATUS/head/count/length,
reserved-continuation flag, copy begin/end, RAW/ST end, take begin/end,
early-versus-taken mismatch count, completed-view reread mismatch count,
preceding scan ordinal, preceding copy begin/end and preceding-versus-taken
mismatch count. The second completed view is not an independent volatile bus
observation. `P` rows contain unit ID, byte offset, up to16 preceding-scan
bytes, then the same number of early and taken bytes. Packet dumps belong
only in local evidence.

This image does not associate a STA, exchange keys or establish HE/A-MPDU
stimulus delivery. A host must characterize and label actual peer traffic;
ordinary beacons or broadcast UDP cannot be represented as the R24 associated
CCMP HE20 one/eight-MPDU stimulus. No negative cause conclusion follows from
an undelivered stimulus. Live acquisition emits no console output.

## Source120 capture (selection160)

One passive channel13 receive window lasts **700 ms**. RX delivery5/14/24 is
enabled and sampled/acknowledged for250ms, then all MAC enables are masked
without acknowledging for the remaining450ms. No CPU source120 handler is
installed: the observation is peripheral-register visibility, not ISR latency.
The journal lives in internal SRAM `.critical.bss.rx_irq120`; it emits no
console traffic during collection. There are no CPU-owned barrier nodes in
this mode. Buffer-full pressure can occur naturally; no frame is synthesized.

After stopping outer RX and the descriptor walker, phases162–169 record:
masked pending, RX unmask, remask, second unmask, selected W1C5 while enabled,
selected W1C24 while masked using a retained enabled STATUS sample, W1C14
while enabled, and final all-enable mask. A zero positive-precondition sample
makes that lane's clear test inconclusive. The selected acknowledgement
consumes an affine STATUS sample narrowed through generated field accessors;
full RAW observations grant no write authority. All other pending causes
remain unacknowledged by these selected clears.

`IRQ120 begin/end` delimit output. Each `I` row contains48 hexadecimal words;
phase IDs160–169 below are decimal (printed as `a0`–`a9`). The accesses are
sequential and perturb timing; each cycle value brackets the adjacent reads.

| Indices (zero-based) | Words |
| --- | --- |
| 0–2 | scan ordinal, phase, begin mcycle |
| 3–8 | BASE, pre LAST/NEXT, pre DONE bitmap0–31, pre R DONE, pre-read end cycle |
| 9–12 | first RAW, RAW-read end cycle, first STATUS, STATUS-read end cycle |
| 13–17 | post LAST/NEXT, post DONE bitmap0–31, post R DONE, post-read end cycle |
| 18–21 | second RAW, its end cycle, second STATUS, its end cycle |
| 22–23 | compact RX enables (5=1,14=2,24=4), full-enable classified Disabled boolean |
| 24–39 | MPDU, END, FCS, abort, abort-FCS-pass, power-drop, same-BM, other-unicast, last-unmatched, buffer-full, FIFO-overflow, NRX-abort, NRX-restart, HT-SIG, HE-SIG-A-CRC, data-success |
| 40–42 | statistics end cycle, pre/post NEXT upper-unknown flags |
| 43–47 | before-ack cycle (0 if no ack), after-ack RAW, after-ack STATUS, compact RX enables, final cycle |

Stopped checkpoint rows have zero in43–46; their first/second RAW/STATUS and
enables remain in9/11/18/20/22. At most256 changed/forced rows are retained;
live collection reserves10 rows for stopped phases and stops on exhaustion,
then dumps the retained evidence before returning a failed verdict.
`N` rows contain node index (hex20=R), CONTROL, BUFFER, descriptor NEXT,
then all32 stopped buffer words. Data dumps belong only in local evidence.

A DONE read after RAW=1 can occur after a later DMA store; it does not alone
prove DONE was already set when RAW was observed. Pre/post DONE and cursor
brackets permit bounded ordering/counterexample analysis. Unchanged payload
at stop is also not a proof that every payload beat preceded IRQ or DONE.
Cause identity requires independently characterized stimulus; counter
co-occurrence alone cannot distinguish the physical roles of5 and24.

## Barrier observer (selection4)

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
