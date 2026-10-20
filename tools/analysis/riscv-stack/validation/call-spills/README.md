# Publisher stack word across a transparent ROM trampoline

Base: fresh `main` **7be12c9e**, including #183. The shared write summaries
transport all twelve `memcpy` store effects through the machine-proved tail
trampoline. The transition itself has no remaining effect barrier. The store
ranges remain unknown, so the publisher spill and subsequent reload remain
unproved. No loop bounds or new trusted ROM memory contracts are used.

## Rediscovered machine facts

Both w0/w1 bundles are built before and after in the same checkout and toolchain.
The fresh w1 image retains the saved addresses:

| Fact | Fresh machine evidence |
| --- | --- |
| Receive future poll body | entry `0x5004d7c8` |
| Publisher snapshot | `0x5004d856: lw ra, 0x104(s3)`, through entry a0 |
| Spill | `0x5004dcf2: sw ra, 0xf8(sp)`, entry-sp minus 600, width 4 |
| Call | `0x5004dd24: jalr ra, 0x7a4(ra)`, target `0x2f8004c4` |
| Actual sp / a0 / a2 at call | entry-sp minus 848 / entry-sp minus 400 / 128 |
| Later reload | `0x5004eb44`, entry-sp minus 600, width 4; still unknown |

The pinned companion is `esp-rom-elfs/20260528/esp32s31_rev0_rom.elf`, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`, as specified
in `chips/esp32s31/vendor/artifacts.toml`. Its global executable assembly entry
`__call_memcpy` at `0x2f8004c4` contains `59d0c06f`, a direct tail jump to
`memcpy` at `0x2f80d260` (size 224). The canonical instruction, transfer and
external CFG edge agree. Its sole architectural register-write record is
`x0 = 0`, and the complete pre-transfer register state equals entry state,
including `sp = entry-sp + 0`. This establishes transparency independently of
the function's name and the decoder's ABI-specific `link=false` classification.
The synthetic `OpaqueCall` at that same instruction is the only semantic gap.

The diagnostic captures each function once through existing `ImageFacts`,
`inspect_function`, the shared decoder, table-following driver and value engine.
It also runs the existing companion/frame-summary validation. It neither walks
another ELF model nor adds a decoder, CFG or memory-effect model.

## Transported effects and remaining uncertainty

The target's closed graph has no unsupported effects or outgoing calls or
transfers. Its twelve stores are:

- byte stores at `0x2f80d27a` and `0x2f80d2c8`;
- word stores at `0x2f80d2f8`, `0x2f80d2fc`, `0x2f80d304`, `0x2f80d308`,
  `0x2f80d30c`, `0x2f80d310`, `0x2f80d314`, `0x2f80d318`, `0x2f80d324` and
  `0x2f80d338`.

`FunctionWrites` for the trampoline contains all twelve events, with their
original PCs, widths, `May` occurrence and `ClosedGraph` proof. Each event and
barrier names the body as its original owner and the proved trampoline transfer
as its provenance. The caller's `CallWrite` additionally retains the trampoline
callee and source summary event. The reproduction checks every transported event
and barrier against the body's original record, not just their counts.

Every store destination remains unknown after the value engine merges the
loop-carried pointers. All twelve `UnknownStore` barriers survive both tail
composition and caller substitution. No `Call` or `ExternalTransfer` barrier
remains at the trampoline. The conservative footprint is still **all RV32 byte
addresses**, including the publisher spill. The missing fact is a complete,
width-aware bounded footprint for every loop-carried store; the zero frame and
unchanged sp cannot establish it.

The actual destination begins at caller entry-sp minus 400, with length 128,
while the spill occupies `[-600,-596)`. Those inputs do not bound all stores
without a separate proof of the body's ranges. The reanalysis therefore still
kills the captured snapshot at `0x5004dd24` and leaves `0x5004eb44` unknown.
No publisher preservation, handler reload or complete IRQ-stack bound follows.

## Composition contract

`function_writes` accepts already available `FunctionWrites` and consumes only
canonical `FunctionIndex` facts. The supported trampoline consists of exactly
one unconditional jump with one proved external edge, no additional actions,
unchanged pre-transfer state and an architectural x0 write. A nonzero link
destination (including a non-ABI link register), any other instruction, changed
argument/sp, condition, unknown semantics or target keeps the transfer barrier.

Composition is bounded to one hop. Missing summaries, incompatible ABI, cycles
and further trampoline chains remain explicitly unknown. Only the proved jump's
`ExternalTransfer` and synthetic `OpaqueCall` observations are removed. Every
target barrier, including internal calls, external transfers, atomics, open
graphs, unsupported effects, unknown stored values and unknown ranges, survives.

The common `substitute` mechanism maps body entry-stack coordinates to the
trampoline's coordinates with equal sp, then through the actual caller sp at
`call_writes`. Field-load PCs and local expression owners remain the body's.
Writes, reloads and barriers retain their source owner and tail proof.
`stack_calls` still rechecks the actual live target and exact sp/argument inputs
before preserving a word. Caller-saved clobbering, the eight-word stack model,
`CallReturns`, `CallBehavior` and the firmware/callback profiles keep their
contracts. All image-dependent storage and proof steps use the existing memory
and run-control budgets.

## Validation and reproduction

The historical comparison below records #185. The current `explain.py` also
checks the subsequent [alignment-loop proof](../alignment-writes/README.md):
three finite byte effects replace one unknown-address effect, while eleven
other store instructions keep their barriers and the reload remains unknown.
It accepts `--before-facts` to compare every other transported effect with a
fresh main diagnostic.

Full captures and check logs are retained under `/tmp/oer-tail-summaries/`.
[real-transition.json](real-transition.json) records the fresh ELF and ROM hashes,
all twelve transported store PCs, the tail proof and the retained uncertainty.
[comparison.json](comparison.json) records the w0/w1 machine, report and callback
candidate comparisons.

Tests and all-target clippy pass for 43 host packages, including reverse
consumers and dependency-layer checks, with `oer-hil-lab/test-support` and serial
tests. Final focused checks pass **179 stack tests and 35 common-analysis
tests**, all-target clippy, formatting and diff checks. Both final firmware
builds and the pinned-ROM reproduction pass. All **14 machine files**, **eight
full reports** (including resource counters) and **four full callback audits**
are byte-identical; every candidate set is unchanged. No manifests, lockfile,
dependency edges or reviewed ROM contracts change.

Regressions cover transparency, argument/sp writes, nonzero architectural link
destinations, conditional paths, missing/unknown targets and summaries, cycles,
unsupported chains, ABI mismatch, incomplete proof facts, stack-coordinate
rebasing, load/store/expression provenance, every body barrier, conditionality,
unknown contents, disjoint/overlapping spill ranges and stale call inputs or
targets. Cancellation at every proof step and failed memory admission release
all temporary/retained allocations.

Build and capture before and after using the
[shared instructions](../callback-address-audit.md#reproduction), then run:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-tail-summaries/before /tmp/oer-tail-summaries/after \
  --output /tmp/oer-tail-summaries/comparison.json
for app in w0 w1; do
  for stage in runtime bootstrap; do
    cmp "/tmp/oer-tail-summaries/before/$app-$stage-callbacks.tsv" \
        "/tmp/oer-tail-summaries/after/$app-$stage-callbacks.tsv"
  done
done
python3 tools/analysis/riscv-stack/validation/call-spills/explain.py \
  /tmp/oer-tail-summaries/after/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-tail-summaries/explained
```

The script asserts the ROM pin, the actual transfer/edge/register-write/input
facts, complete event/barrier transport and unknown reload. Fixture PCs select
and check the fresh image only; no summary semantics depend on them. The public
diagnostic accepts arbitrary call PCs and stack slots:

```sh
target/tools/examples/stack-call-writes ELF ROM_ELF \
  chips/esp32s31/platform/rom-functions.toml CALL_PC_HEX SLOT_DECIMAL
```
