# One bounded ROM alignment-store footprint

Baseline: fresh main `8db49c49`, including #183 and #185. Both w0/w1 bundles
were built before and after in the same checkout. Full diagnostics, bundles and
check logs are retained under `/tmp/oer-alignment-writes/`.
[real-transition.json](real-transition.json) records the pinned ROM, actual
caller inputs, proof, twelve-instruction inventory and retained uncertainty.
[comparison.json](comparison.json) records every machine/report/audit comparison.
PCs and ROM hashes select the reproducible fixture; no proof rule reads a name,
selected PC, hash or trusted ROM memory contract.

## The proof and its limits

The general value pass already owns the decoded `SemanticOp`s, instruction CFG,
branch semantics and converged register states. Its supplementary structural
proof neither executes sample inputs nor introduces a second interpreter or
resolver CFG. The proof requires:

- A contiguous straight body and one masked nonzero back edge, with exactly
  the expected fallthrough/taken edges and no call, relocation ambiguity,
  unsupported effect or conflicting instruction in the cycle.
- Every incoming edge to every cycle instruction is checked. Exactly one
  external edge is supported, and it must enter the shared mask/test block.
  Direct entry to the body, a bypass of the mask or an additional predecessor
  rejects the proof, even when one preferred path looks safe.
- The pointer's initial value is the existing fixed point's all-path value at
  that external predecessor. The predecessor must preserve it. Every pointer
  destination in the cycle is checked: exactly one `p = p + 1` is allowed.
- The test is `p & mask`, followed by `bne residue, zero, body`, with no pointer
  change in the test. Supported masks are 1, 3 and 7; other forms stay unknown.

Let `A = mask + 1`. At a test, `p = initial + n (mod 2^32)` and `r = p & mask`.
`A` divides `2^32`, so wrapping addition has exactly the same residue transition
as ordinary addition modulo `A`. For `r = 0` the body is skipped. Otherwise the
rank `A - r` is in `1..=mask`; the sole `+1` decreases it by one, until the next
zero test exits. Thus there are zero through `mask` iterations for **every**
RV32 initial word. No alignment of the original address is assumed.

In the real ROM, `0x2f80d2d4` initializes a5 from entry a0 and
`0x2f80d2d6` jumps to the mask at `0x2f80d2cc`. The test at `0x2f80d2d0`
therefore precedes the first body execution. The only a5 update is `+1` at
`0x2f80d2c4`. Store `0x2f80d2c8` occurs after that increment and uses `-1`,
so its possible addresses over all residues are:

```text
entry_argument0 + {0, 1, 2} (mod 2^32)
```

The common `StoreAddressRange` fact retains the initial value, normalized
displacement, bound and `AlignmentLoopProof` PCs/register/mask. `FunctionWrites`
only expands that fact into three existing byte-write effects. Every effect
keeps `May`, the original store PC, unknown byte contents and the induction
proof. An open local graph discards the addresses and proof. This is a superset
of possible events, not three writes on every invocation or final memory values.

Existing #185 composition transports the events through `__call_memcpy` at
`0x2f8004c4`. Existing #183 argument substitution uses the actual call at
`0x5004dd24`: a0 is caller-entry-sp minus 400, a2 is 128. The three bytes become
`[-400,-399)`, `[-399,-398)`, `[-398,-397)` in the caller's stack coordinates.
The existing RV32 stack-range exclusion query proves each disjoint from the
spill `[-600,-596)`. The proof and original body owner survive both boundaries.

## All twelve store instructions

| ROM store PC | Bytes | Address proof / remaining barrier |
| --- | ---: | --- |
| `0x2f80d27a` | 1 | `UnknownStore`: length/end comparison loop has no supported finite bound |
| `0x2f80d2c8` | 1 | a0 + `{0,1,2}`; three `May` effects, `UnknownValue` contents |
| `0x2f80d2f8` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d2fc` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d304` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d308` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d30c` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d310` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d314` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d318` | 4 | `UnknownStore`: bulk-loop pointer, no supported finite bound |
| `0x2f80d324` | 4 | `UnknownStore`: bulk-loop pointer after its +36 update, no supported finite bound |
| `0x2f80d338` | 4 | `UnknownStore`: word-loop pointer, no supported finite bound |

There are fourteen effects for twelve instructions. The other eleven
instructions' writes and barriers are unchanged byte for byte in the diagnostic,
including their original owner and trampoline provenance. Together their
unknown footprint still includes all RV32 addresses. The whole call therefore
does **not** preserve the spill by proof; reload `0x5004eb44` remains unknown.
There is no new trusted ROM contract and no proof for those other loops.

## Checks and comparison

Regressions use arbitrary PCs and different pointer registers, all four residue
classes, exact ABI argument substitution near `0xffffffff`, both store/update
orders and signed displacements. Negative fixtures cover an initial-check
bypass, an extra predecessor entering the body or the test, changed/variable
steps, another pointer definition, a different test pointer/mask/condition,
halfword/word stores, calls, unsupported effects, an opaque initial pointer and
an open local graph. Cancellation at every analysis and effect-expansion step,
and failed memory admissions, release all retained/scratch storage. The symbolic
induction and the exhaustive residue quotient justify the bound; concrete ABI
inputs test its transport and wrapping arithmetic.

All **14 machine files** and **four complete callback-audit TSVs** are byte
identical. Candidate sets and unknown-use classes are unchanged. All **eight
reports** are semantically identical: only the `analysis:` work counter differs
in four stack reports. Peak accounted memory is unchanged.

| Image | Work units before | Work units after | Peak bytes, unchanged |
| --- | ---: | ---: | ---: |
| w0 runtime | 77,832,758 | 78,325,575 | 156,792,327 |
| w1 runtime | 99,940,345 | 100,515,443 | 158,194,248 |
| w0 bootstrap | 15,720,768 | 15,728,878 | 16,357,382 |
| w1 bootstrap | 15,720,140 | 15,728,250 | 16,472,074 |

No manifests, dependency edges, lockfile or reviewed ROM contracts change.
Tests and all-target clippy complete successfully for 46 host packages selected
from the changed layers' reverse dependency graph plus the common ELF/RV32
foundations and dependency-layer tests, using `oer-hil-lab/test-support` and
serial test execution. Final focused checks pass 188 stack tests (nine new),
35 common-analysis tests, 20 model tests and five dependency-layer checks.
Formatting, diff checks and the real-ROM validator pass.

## Reproduction

Build/save before and after bundles and audits using the
[shared capture instructions](../callback-address-audit.md#reproduction), then:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
cargo test --locked -p oer-analysis-riscv-model -p oer-analysis-riscv-analysis \
  -p oer-analysis-riscv-stack -p oer-test-layers
cargo fmt --all -- --check
cargo clippy --locked --all-targets -p oer-analysis-riscv-model \
  -p oer-analysis-riscv-analysis -p oer-analysis-riscv-stack
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-alignment-writes/before /tmp/oer-alignment-writes/after \
  --output /tmp/oer-alignment-writes/comparison.json
python3 tools/analysis/riscv-stack/validation/call-spills/explain.py \
  /tmp/oer-alignment-writes/after/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-alignment-writes/explained \
  --before-facts /tmp/oer-alignment-writes/before-facts.tsv
```

The baseline `before-facts.tsv` comes from main's `stack-call-writes` executable
with the same ELF/ROM, summaries, call PC `5004dd24` and slot `-600`. The validator
uses canonical diagnostic observations, checks the pin and complete instruction
inventory, all fourteen transported effects and every remaining barrier, each
finite byte's spill exclusion and the still-unknown full-call reload.
