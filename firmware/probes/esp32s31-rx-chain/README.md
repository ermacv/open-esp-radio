# S31 RX chain hardware observation

This probe runs selections **200–209** for one bounded source120
traffic/take/recycle observation, using the public cold hardware transitions
and a retained 32×128-byte RX arena plus R.

## Traffic and ownership capture (selections200–209)

Selection200 runs one 700-ms maximum RX window, or stops at twelve taken
units. It uses the existing permanent 32×128-byte arena plus R. Every MMIO
operation borrows the public HAL owner. There is no source120 CPU handler;
timestamps describe peripheral visibility, not ISR entry. Nothing in production
calls the independent opaque-enable observation added for this measurement.

The probe calls the existing backend's public `prepare`, `enter` and
`track` transitions directly on P2, in production order, using placed futures.
A `ClaimCell` transfers only the resulting permanent running owner's reference
to P1. The backend, clocks and running owner remain retained until the leased
host reset; no reusable radio is returned and no independent PAC claim is made.

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
RAW and the exact pre-ack STATUS sample, then copies them again immediately
after those samples. The existing observation checks use
the original pre-RX BUFFER bindings. The taken unit's complete segments are
copied and compared against those earlier bytes and the preceding scan's
retained byte copy and the post-IRQ byte copy, then read again through the
completed view before recycling. A post-IRQ copy differing from the taken
unit can supply a counterexample only with the corresponding completion and
fresh14 transition identified; equality is a bounded sample, not a bus proof.
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
mismatch count, post-IRQ copy begin/end and post-IRQ-versus-taken mismatch
count. The second completed view is not an independent volatile bus
observation. `P` rows contain unit ID, byte offset, up to16 preceding-scan
bytes, then the same number of early, post-IRQ and taken bytes. Packet dumps belong
only in local evidence.

This image does not associate a STA, exchange keys or establish HE/A-MPDU
stimulus delivery. A host must characterize and label actual peer traffic;
ordinary beacons or broadcast UDP cannot be represented as the R24 associated
CCMP HE20 one/eight-MPDU stimulus. No negative cause conclusion follows from
an undelivered stimulus. Live acquisition emits no console output.

Before BASE publication, the probe initializes buffer words with distinct
patterns; buffers12/20/28 keep full patterns and other buffers keep the leading
marker. RX descriptors stay armed; there are no CPU-owned barrier nodes.
These bytes are never painted during RX.

Build with `cargo fw probe oer-firmware-probes-esp32s31-rx-chain`. The staged
ELF requires the normal bootstrap/PSRAM handoff. Under a board/air lease:

```console
cargo stand --owner wifi lease --board s31-b --air exclusive -- \
  cargo fw run ../probes/esp32s31-rx-chain --board s31-b --timeout 8s
```

The image retains exception/panic details through the runtime's public hooks;
a subsequent boot prints the retained diagnostic and a failed verdict.
No handler takes the live console's owner. The direct-backend composition
passes mandatory image gates with the existing interrupt-stack allocation;
no panic budget or memory profile is raised. A pass denotes bounded collection,
not a hardware ordering or traffic-delivery qualification.
