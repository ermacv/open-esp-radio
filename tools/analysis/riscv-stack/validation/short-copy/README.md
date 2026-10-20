# Contextual reachability for the real 20-byte memcpy call

Fresh baseline: main `d32c4761`, including the corrected graph closure from
#191. The ROM pin is SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
[proof.json](proof.json) contains the verified caller context, shared alignment
witness, finite branch operands, region entrances, and all twelve store sites.
[comparison.json](comparison.json) records every machine, report and callback
audit comparison. Full before/after bundles, canonical streams and check logs are retained under
`/tmp/oer-short-copy/`. Addresses and the pin select this validation fixture;
production proof rules use no function names, ROM addresses or length literals.

## Actual inputs and arithmetic

The fresh w1 runtime confirms the call at `0x5004db66` targets `0x2f8004c4`.
The existing transparent-tail proof resolves that trampoline to `0x2f80d260`.
The caller's canonical `CallInputs` proves entry **a2 = 20**; **a0 is unknown**.
The historical `-600` is only the diagnostic's requested word coordinate. It
identifies no current spill and supplies no preservation premise.

The machine computes the end from original a0 and **entry** a2 before a2 is
overwritten. #191 supplies the all-input equality for a5 at `0x2f80d28c`.
The shared evaluator then computes:

```text
D = original destination
P = (D + 3) & 0xfffffffc           // #191 align-up, modulo 2^32
E = (D + entry_length) & 0xfffffffc
R = E - P                        // modulo 2^32
live a2 = 32                     // instruction at 0x2f80d294
signed(live a2) < signed(R)       // branch at 0x2f80d298
```

The bounded arithmetic rule matches two masked affine expressions with the
same base and mask. For `A = mask + 1` dividing `2^32`, write `D = Q + r`,
where `Q` is aligned and `r` ranges over **every** residue. In RV32 modular
arithmetic, masking distributes over adding Q, and Q cancels in subtraction:

```text
((D + a) & !mask) - ((D + b) & !mask)
  = ((r + a) & !mask) - ((r + b) & !mask)  (mod 2^32)
```

The arithmetic is limited to masks 1, 3 and 7 (at most eight residues).
Unrecognized expressions keep both branch edges. The mask comes from the common expression DAG, the offset 20 from the proven
call input, the offset 3 from #191's machine witness, and the threshold 32 from
its actual instruction. There is no premise excluding arithmetic overflow.

| D mod 4 | P - D, modular | E - D, modular | R |
| --- | --- | --- | --- |
| 0 | 0 | 20 | 20 |
| 1 | 3 | 19 | 16 |
| 2 | 2 | 18 | 16 |
| 3 | 1 | 17 | 16 |

Both possible residuals are positive signed RV32 words below 32, so the
actual signed branch is false. Signedness is applied after deriving the
modular operands; unsigned branches use their own machine predicate.

## Every entrance to the excluded region

The existing CFG has 23 instructions in `[0x2f80d2d8, 0x2f80d332)`, covering
the bulk loads, nine stores, pointer updates, test and final exit jump.
There is exactly **one** edge into this region from outside:

```text
0x2f80d298 -> 0x2f80d328, Taken   // false in this context
```

The only incoming edge to the first bulk load is:

```text
0x2f80d32c -> 0x2f80d2d8, Taken  // internal back edge
```

The optional value pass traverses the same instruction nodes, edges and
prepared operations using the existing evaluator and join. It starts solely
at function entry. Unknown predicates retain both edges. A cycle supplies no
new reachability root. The pass also audits every edge from the reached set
into the excluded set and requires a false branch for each such edge.
The validator independently checks all region entrances and repeats graph
reachability with only the emitted branch cuts. Adding a reachable entrance
retains the bulk effects even when the original guard remains false.

The pass consumes the common `closes_local_graph` result accumulated from
both decoding gaps and edges. Reachable unsupported instructions and unknown
non-call edges suppress specialization. `Unvisited` bytes preserve closure.
#191 equalities strengthen transfer inputs while the incoming join states
remain monotone; no second instruction interpreter or CFG is introduced.

## Preconditions and preserved effects

`call_context` derives constants through the existing `CallInputs` and ABI
substitution. Its fields are private. `contextual_function_writes` runs the
common engine, retains all generic writes/barriers, and attaches a private
conditional exclusion certificate. `call_writes` selects the actual
`Transfer` targets, checks ABI and every required entry constant on **every**
application, then omits only effects whose original owner/site is proved
unreachable. A mismatch uses the complete generic effects. The existing
single transparent tail transports both conditions and original ownership.

`StackCalls` also requires exact live leaves for all constant preconditions
before caching a specialized application. Its ordinary live-input checks
therefore cover the length dependency even if every store was excluded.
An expression normalized to a constant may support an immediate application,
but this bounded cache does not retain that expression dependency.

| Store PC | Bytes | Generic function summary | This call |
| --- | ---: | --- | --- |
| `0x2f80d27a` | 1 | `UnknownStore` | Retained, unknown |
| `0x2f80d2c8` | 1 | Three `May` byte effects at argument0 + {0,1,2}, #188 | All three retained; caller destination unknown |
| `0x2f80d2f8` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d2fc` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d304` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d308` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d30c` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d310` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d314` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d318` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d324` | 4 | `UnknownStore` | Excluded: unreachable |
| `0x2f80d338` | 4 | `UnknownStore` | Retained, unknown |

The generic fourteen effects and their barriers remain byte-identical to
main. The contextual application retains five effects from three sites.
Unknown length, a changed required constant, missing ABI or changed target
cannot reuse the exclusions. The three #188 byte effects retain their `May`
occurrence and proof; their addresses become unknown at this caller.
The other two unbounded sites remain unknown. This establishes neither
publisher preservation nor preservation of any concrete spill, and is not a
complete memcpy memory-effect contract. No trusted ROM contracts are added.

## Regression coverage and reproduction

Nine end-to-end groups cover the real proof shape, lengths from 0 through
reachable bulk cases (including 36, 64 and 128), signed-boundary lengths,
every address residue, RV32 and signed-boundary wrap, signed/unsigned machine
comparisons, an additional region entrance, unproved alignment inputs,
unknown arguments/ABI/targets, changed contexts and target alternatives, unsupported instructions,
unvisited bytes, and reapplication through `StackCalls` including normalized
expression inputs. A common-engine unit test covers all six branch predicates.
Every checkpoint cancellation and several admission budgets verify release
of temporary and retained proof allocations.

Tests and all-target clippy pass for 46 affected host packages, including
reverse dependencies and the dependency-layer gate, with
`oer-hil-lab/test-support` and serial execution. The focused packages pass
206 stack tests, 36 common-analysis tests, 21 model tests and six dependency
checks. Formatting and diff checks pass. Both w0/w1 builds pass. All fourteen
machine files, all eight complete reports and all four complete callback-audit
TSVs are byte-identical to the fresh main baseline; candidate sets, unknown-use
classes, resource counters and accounted peaks are unchanged.

Capture fresh before/after firmware and the four complete callback audits
using the [shared reproduction](../callback-address-audit.md#reproduction).
Preserve baseline `stack-call-writes` and `function-records` executables and
capture their complete streams before changing the common engine. Then run:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
cargo test --locked -p oer-analysis-riscv-analysis -p oer-analysis-riscv-stack \
  -p oer-analysis-riscv-model -p oer-test-layers
cargo clippy --locked --all-targets -p oer-analysis-riscv-analysis \
  -p oer-analysis-riscv-stack -p oer-analysis-riscv-model
cargo fmt --all -- --check
python3 tools/analysis/riscv-stack/validation/short-copy/explain.py \
  /tmp/oer-short-copy/after/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-short-copy/explained \
  --before-records /tmp/oer-short-copy/before-rom-records.tsv \
  --before-effects /tmp/oer-short-copy/before-facts.tsv
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-short-copy/before /tmp/oer-short-copy/after \
  --output /tmp/oer-short-copy/comparison.json
```
