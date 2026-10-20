# Alignment-loop exit value at the real ROM merge

Baseline: fresh main `97ffda65`, including #188 and #178. Before/after firmware
bundles, complete record/effect streams and check logs are retained under
`/tmp/oer-alignment-exit/`. [proof.json](proof.json) records the exact expression
definitions, both incoming edges, their machine chain and the common witness.
[comparison.json](comparison.json) records every machine/report/audit comparison.
Names, PCs and the ROM pin select this diagnostic fixture; production proof
rules use none of them.

## One witness, two conclusions

The #188 structural induction checker is now called once per candidate byte
store after the ordinary value fixed point. Its returned witness contains the
initial `Value`, store displacement, exit edge and existing
`AlignmentLoopProof`. Both `StoreAddressRange` and the exit equality consume
that witness. Entry auditing, the unique `+1` definition, mask and matching
nonzero back edge have one implementation. There is no additional decoder,
loop recognizer, instruction interpreter or CFG.

For `A = mask + 1` and residue `r = initial & mask`, the rank proof gives
`d = 0` when `r = 0`, otherwise `d = A - r`. Thus `d` is in `0..=mask` and
the exit pointer is `initial + d (mod 2^32)`. Because `A` divides `2^32`, this
is the unique aligned word in that forward modular interval, including across
the RV32 boundary:

```text
pointer_exit = initial.wrapping_add(mask) & !mask
pointer_exit & mask = 0
pointer_exit = initial + d (mod 2^32), d in 0..=mask
```

This also covers an already-aligned initial word, where the first test exits
without executing the store. The initial value comes from the existing
all-path state at the audited incoming predecessor; it is never replaced with
an assumed function argument.

## Common representation and propagation

The value is two existing `Expression::Integer` nodes, Add followed by And.
`AlignedValue` is a supplementary model record with:

- `owner`: the function that owns the expression IDs and machine witness;
- `point`: `ValuePoint::Edge { from, target }` or `Entry { offset }`;
- `register`, original `initial`, normalized `value` and shared loop `proof`;
- `origin`: loop exit, zero-residue branch, propagation or all-input intersection.

Its contract includes the alignment and finite modular displacement above,
using `proof.mask`. This is an equality **if the edge/entry is reached**, not a
reachability assertion or an instruction's architectural register write.
Constant contradictory branch predicates may describe infeasible edges; facts
on them remain conditional and do not establish an executing path.

A small equality component tracks which registers equal this expression over
the **existing** CFG. It starts without equalities at function entry and joins
with intersection at every incoming path. Loop exits and matching zero-residue
conditions seed it. A path without proof contributes no equality. Transfers
reuse the common `evaluate` with the proved register equalities; copies retain
them and changed results lose them. Calls, unsupported effects and ambiguous
relocations clear them. The existing graph-closure predicate rejects unknown
non-call control flow. Facts are emitted only after this fixed point.

This preserves a relation through copies and jumps to the next merge while
leaving the conservative scalar `Value` and `MemoryAccess` observations intact.
No memory contents are forwarded from the derived pointer fact. The new proof
payload is boxed and its allocations are admitted, so adding the record does
not enlarge every ordinary function record. Temporary equality states/queue,
expression interning, retained facts and every traversal use the existing
working-memory and run-control budgets. Duplicate stores sharing one loop
witness do not duplicate equality propagation.

## Fresh ROM machine chain

The pinned body remains `0x2f80d260`, the store is `0x2f80d2c8`, and the merge
is the entry of `0x2f80d28c`. The shared graph confirms exactly two incoming
edges at that merge, from `0x2f80d28a` and `0x2f80d2d2`.

| Path / stage | Actual machine facts | Derived relation |
| --- | --- | --- |
| Outer alignment test | `0x2f80d284: andi a5,a0,3`; `0x2f80d288: bne a5,zero,…` | Fallthrough to `0x2f80d28a` proves original a0 is aligned |
| Already-aligned copy | `0x2f80d28a: add a5,zero,a0` | On edge to `0x2f80d28c`, a5 equals align-up(original a0,4) |
| Loop initialization | `0x2f80d2d4: add a5,zero,a0`; `0x2f80d2d6: j -10` | Initial is original a0; entry goes to the shared test at `0x2f80d2cc` |
| Loop body | Unique update `0x2f80d2c4: addi a5,a5,1`; byte store `0x2f80d2c8: sb a4,-1(a5)` | #188 footprint stays original a0 + `{0,1,2}`, `May` |
| Loop test / exit | `0x2f80d2cc: andi a4,a5,3`; `0x2f80d2d0: bne a4,zero,-16` | Only the fallthrough edge `0x2f80d2d0 → 0x2f80d2d2` supplies the exit equality |
| Exit jump | `0x2f80d2d2: j -70` | Equality survives to `0x2f80d28c` |
| Merge | Both incoming edges prove the same initial, value and witness | a5 equals align-up(original a0,4) on every incoming path |

The exact local DAG on this fixture is:

```text
e9   = EntryRegister { register: 10 }                 // original a0
e103 = Integer { op: Add, left: e9, right: 3 }
e104 = Integer { op: And, left: e103, right: 0xfffffffc }

AlignedValue {
  owner: 0x2f80d260,
  point: Entry { offset: 0x2f80d28c },
  register: 15,
  initial: e9,
  value: e104,
  proof: AlignmentLoopProof {
    header: 0x2f80d2c0, guard: 0x2f80d2cc,
    back_edge: 0x2f80d2d0, step: 0x2f80d2c4,
    entry_edge: 0x2f80d2d6, pointer: 15, mask: 3
  },
  origin: AllIncoming
}
```

Thus at the checkpoint:

```text
a5 = align_up_32(argument0, 4)
a5 & 3 = 0
a5 = argument0 + d (mod 2^32), d in {0,1,2,3}
```

The ordinary copy's `Value` remains e9 at `0x2f80d28a`. The branch at
`0x2f80d2d0` has no architectural a5 write. The jump at `0x2f80d2d2` writes
only x0. The new facts explicitly carry the control point instead of inventing
an a5 definition at either control instruction.

## Preserved uncertainty and fresh caller

All fourteen write effects for all twelve ROM store instructions and every
ROM/call barrier are byte-identical to the fresh main diagnostic. The three
byte addresses from #188 retain their proof and `May`; the other eleven
instructions retain `UnknownStore`. No trusted ROM contract is added.

Main #178 changed firmware layout: historical call `0x5004dd24` is no longer
a transfer. The new validation rediscovers a real memcpy call at `0x5004db66`
in the fresh w1 run function and checks its actual target/inputs and complete
trampoline effect transport against baseline. Its a0 is unknown to the common
analysis. The full call still cannot prove word preservation. `-600` in that
query is only a requested word coordinate; this task does not identify a new
spill or reuse the former caller's layout.

## Tests and analysis cost

Tests and all-target clippy pass for 46 affected host packages, including reverse
dependencies, ELF/RV32 foundations and dependency-layer checks, with
`oer-hil-lab/test-support` and serial execution. There are seven new end-to-end
groups: arbitrary PCs/registers and nonzero initial offsets; all residues and
RV32 wrap; rejected cycle conditions; incompatible/unproved merge inputs;
post-exit copies/changes; an open graph; and multiple stores sharing one exit
propagation. Existing footprint regressions
remain. The cancellation/admission test now uses the two-path join fixture and
checks every analysis step; the model additionally tests serde round-trip and
boxed-payload accounting. Formatting and diff checks pass.
Final focused checks pass 195 stack tests, 35 common-analysis tests, 21 model
tests and six dependency-layer checks.

All **14 machine files** and **four complete callback-audit TSVs** are byte
identical. Every candidate set and unknown-use class is unchanged. All **eight
reports** are semantically identical; only the work-unit counter differs in
four stack reports. The extra candidate collection and equality traversal are
metered work. Peak accounted memory is unchanged because other functions still
dominate the pipeline peak; the new local facts are admitted and released.
The work-unit increase is approximately 0.51–0.67% across these four analyses.

| Image | Work units before | Work units after | Peak bytes, unchanged |
| --- | ---: | ---: | ---: |
| w0 runtime | 78,319,813 | 78,742,157 | 156,646,584 |
| w1 runtime | 100,788,310 | 101,302,649 | 158,320,039 |
| w0 bootstrap | 15,728,878 | 15,833,645 | 16,357,378 |
| w1 bootstrap | 15,728,365 | 15,833,130 | 16,468,560 |

No manifests, dependency edges, lockfile or reviewed ROM contracts change.

## Reproduction

Capture fresh before/after bundles and full audits with the
[shared instructions](../callback-address-audit.md#reproduction). Keep baseline
`function-records` and `stack-call-writes` executables, then capture:

```sh
ROM="$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf"
/tmp/oer-alignment-exit/before-function-records "$ROM" 2f80d260 \
  > /tmp/oer-alignment-exit/before-rom-records.tsv
/tmp/oer-alignment-exit/before-stack-call-writes \
  /tmp/oer-alignment-exit/before/w1/runtime.elf "$ROM" \
  chips/esp32s31/platform/rom-functions.toml 5004db66 -600 \
  > /tmp/oer-alignment-exit/before-facts.tsv

cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
cargo test --locked -p oer-analysis-riscv-model -p oer-analysis-riscv-analysis \
  -p oer-analysis-riscv-stack -p oer-test-layers
cargo fmt --all -- --check
cargo clippy --locked --all-targets -p oer-analysis-riscv-model \
  -p oer-analysis-riscv-analysis -p oer-analysis-riscv-stack

python3 tools/analysis/riscv-stack/validation/alignment-exit/explain.py \
  /tmp/oer-alignment-exit/after/w1/runtime.elf "$ROM" \
  /tmp/oer-alignment-exit/explained \
  --before-records /tmp/oer-alignment-exit/before-rom-records.tsv \
  --before-effects /tmp/oer-alignment-exit/before-facts.tsv
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-alignment-exit/before /tmp/oer-alignment-exit/after \
  --output /tmp/oer-alignment-exit/comparison.json
```

The validator reads the common diagnostic streams. It verifies the ROM pin,
actual instructions and incoming edges, initial/DAG ownership, the exact
merged fact, the shared footprint witness and unchanged full-call effects.
Its fixture addresses and expected IDs never select production proof behavior.
