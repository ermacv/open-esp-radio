# Publisher stack word across the ROM copy call

Base: fresh `main` **44324870**, including #180 and #179. Both w0/w1 bundles
were rebuilt before and after in the same checkout with the same toolchain.
The outcome is a **precise missing fact**, not a proved ROM memory contract.
The first publisher barrier remains the copy call. The new generic mechanism
preserves words only where complete existing write summaries prove exclusion.

## Rediscovered machine facts

The fresh w1 image has the same instruction addresses as the saved #180 image:

| Fact | Fresh machine evidence |
| --- | --- |
| Receive future poll body | entry `0x5004d7c8` |
| Publisher snapshot | `0x5004d856: lw ra, 0x104(s3)`, through entry a0 |
| Spill | `0x5004dcf2: sw ra, 0xf8(sp)`, entry-sp minus 600, width 4 |
| Call | `0x5004dd24: jalr ra, 0x7a4(ra)`, target `0x2f8004c4` |
| Actual sp / a0 / a2 at call | entry-sp minus 848 / entry-sp minus 400 / 128 |
| Later reload | `0x5004eb44`, entry-sp minus 600, width 4; still unknown |

The instruction spellings above are checked against the linked ELF using
`llvm-objdump`; the reusable proof consumes canonical machine records instead.
`CallInputs` leaves a1 symbolic. The pointer snapshot is a field load at its
actual PC, not a claim about the future's entry memory or an exact allocation.
[real-transition.json](real-transition.json) records the ELF hash, actual ABI
inputs, original load PC, spill and unknown reload.

The pinned companion is `esp-rom-elfs/20260528/esp32s31_rev0_rom.elf`, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`, as specified
in `chips/esp32s31/vendor/artifacts.toml`. Its global executable assembly entry
`__call_memcpy` at `0x2f8004c4` contains `59d0c06f`, a direct **tail jump** to
`memcpy` at `0x2f80d260` (size 224). This identity comes from the actual transfer
and companion symbol catalog, not from the call's source name. The diagnostic
also runs the existing joint stack analysis and reviewed ROM summaries; those
summaries describe frames and transfers, not memory noninterference.

## Complete conservative effect and the missing proof

The shared decoder/CFG visits a closed graph of the 224-byte body, with no
unsupported effects or outgoing calls/transfers. The trampoline's single tail
jump is the only transitive transfer. There are twelve store instructions:

- byte stores at `0x2f80d27a` and `0x2f80d2c8`;
- word stores at `0x2f80d2f8`, `0x2f80d2fc`, `0x2f80d304`, `0x2f80d308`,
  `0x2f80d30c`, `0x2f80d310`, `0x2f80d314`, `0x2f80d318`, `0x2f80d324` and
  `0x2f80d338`.

The body has no sp write or explicit stack access and no child call with an
unaccounted frame. Nevertheless **every store base is unknown after the common
value engine merges the loop-carried pointers**. `FunctionWrites` therefore
retains twelve `UnknownStore` barriers. Its conservative write footprint is
**all RV32 byte addresses**, which includes the publisher spill. The existing
observed zero frame cannot shrink this footprint.

The exact missing fact is a loop invariant bounding **all twelve stores**,
including each store's width, alignment prefix, unrolled 36-byte copies,
word/byte tails and RV32 arithmetic, followed by transporting that complete
footprint through the trampoline's tail transfer. `call_writes` currently
retains external transfers rather than composing a tail callee's effects;
clearing that barrier alone would still leave all twelve unknown ranges.
Neither the function name nor unchanged sp proves the invariant.

A possible **additional trusted contract**, if separately reviewed and
explicitly supplied, would state that this pinned trampoline/body writes only
the RV32 modular byte range beginning at entry a0 with length entry a2, with
no other writes, including stack or transitive writes. That is a new premise,
not the current frame/calls TOML contract. **No such contract is installed,
parsed or enabled by this change.** Under that extra premise only, substitution
would yield `[-400,-272)`, disjoint from the spill `[-600,-596)` in the caller's
entry-stack coordinates. This conditional arithmetic is not presented as a
machine proof.

The real diagnostic reanalyzes the caller with prepared summaries and confirms
that the spill still records the snapshot from `0x5004d856`, but its knowledge
is killed at **`0x5004dd24`**. It therefore cannot advance to a later barrier or
prove the reload at `0x5004eb44`. No claim about `MAC_SIGNAL`, `RX_LINE`, handler
publication or the complete IRQ-stack bound follows.

## Reused mechanisms and narrow extension

- `FunctionWrites` still records all store events and graph/semantic barriers
  from `FunctionIndex`. `UnknownValue` distinguishes opaque stored contents
  from an unknown destination; only the former can be ignored for exclusion.
  Both still report unknown memory contents to existing summary consumers.
- `call_writes` still substitutes actual integer ABI arguments and unions every
  `Transfer` alternative. It now translates callee-owned entry-stack addresses
  through the call's actual sp. Missing targets/summaries/ABI, open graphs,
  calls, external transfers, atomics and unsupported effects fail closed.
- `CallWrites::preserves_stack_word` requires every possible store's complete
  byte range to be disjoint. Only matching caller entry-stack coordinates
  prove exclusion; absolute or foreign/symbolic addresses cannot exclude an
  unknown absolute stack. Checked arithmetic and signed RV32 coordinate limits
  reject overflowing/possibly modular-aliasing ranges. No writes are inferred
  from `May`, `Must`, frame sizes or a function name.
- `stack_calls` prepares admitted, caller-scoped applications from canonical
  facts. The optional `ResearchOptions::stack_calls` rechecks the target and
  exact sp/argument leaves at the live call before preserving a word. Changed
  inputs/targets/caller invalidate the application. Inputs used for a known
  stack destination must be exact stack leaves; unknown stored contents do not
  constrain the footprint. It caches no mutable fields or expression IDs.
- The value solver retains only its existing eight entry-stack words, original
  captured values/load PCs, width/overlap invalidation and intersection joins.
  Register clobbering, `CallReturns` and `CallBehavior` keep their contracts.
  Default research, callback and firmware profiles supply no preservation facts.

`ImageFacts`, companion loading, the structural decoder, local CFG,
`inspect_function`, table-following driver and memory/control ledgers are reused.
There is no new decoder, CFG, ELF/DWARF reader or parallel effect model. ROM
summary parsing remains unchanged because no new memory premise is admitted.

## Validation and reproduction

Full logs and captures are in `/tmp/oer-call-spill/`. The checked-in
[comparison](comparison.json) records **14 byte-identical machine files**, all
**eight byte-identical reports** (including resource counters), and four
**byte-identical full callback TSVs**. Every per-site candidate set is unchanged;
there are no candidate changes requiring separate proofs.

Tests and clippy passed for 43 host packages: the analysis layers and their
reverse dependencies, shared ELF/RV32 foundations and dependency-layer tests,
with `oer-hil-lab/test-support` and serial tests. After the final input-revalidation
regression, the affected packages were checked again: **171 stack tests and 34
common-analysis tests**, all-target clippy and formatting/diff checks pass. No
manifests, lockfile or dependency edges changed. Both final firmware builds and
the pinned real diagnostic also pass.

Regression coverage includes disjoint argument writes (even unknown contents),
partial/byte overlap, callee stack writes with actual sp substitution, unknown
ranges and targets, internal calls/atomics, complete multi-target unions and a
missing/overlapping alternative, changed proof inputs/target/caller, rejection of normalized symbolic address
arguments without exact live constraints, RV32 and
host arithmetic limits, caller-saved clobbering, original load PCs, cancellation
and allocation-failure cleanup.

Build/capture before and after using the
[shared instructions](../callback-address-audit.md#reproduction), then run:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-call-spill/before /tmp/oer-call-spill/after \
  --output /tmp/oer-call-spill/comparison.json
for app in w0 w1; do
  for stage in runtime bootstrap; do
    cmp "/tmp/oer-call-spill/before/$app-$stage-callbacks.tsv" \
        "/tmp/oer-call-spill/after/$app-$stage-callbacks.tsv"
  done
done
python3 tools/analysis/riscv-stack/validation/call-spills/explain.py \
  /tmp/oer-call-spill/after/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-call-spill/explained
```

The script asserts the platform ROM hash and machine-derived target, complete
store inventory and retained uncertainty. Fixture PCs select/check the fresh
image only; no resolver or summary semantics depend on them. For arbitrary
inputs, the public diagnostic takes caller call PC and stack offset as arguments:

```sh
target/tools/examples/stack-call-writes ELF ROM_ELF \
  chips/esp32s31/platform/rom-functions.toml CALL_PC_HEX SLOT_DECIMAL
```
