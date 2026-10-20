# Split dispatch and CPU0 stack context

On main `4901ee7d` (#222, including the `6ed51a2b` Installed/Entry fix),
the general analyzer proves all **30** targets of the indirect jump at
**`0x50065a40`** in main **`0x5006553a`**. Its emitted CFG passes the existing
`writes::closes_local_graph` check. The fresh w1 runtime ELF SHA-256 is
`dc1ff3ae9e5e007439d5b15310d7b6243276efae1b7dad90873f2e755f357571`.
The #222 fixture used `0x500659fc` in `0x500654f6`; these addresses only select
diagnostic input and supply no production proof rules.

[proof.json](proof.json) records every relocated immutable word, the complete
target set, the actual checked stack contexts and stream hashes.
[check.py](check.py) captures and validates the evidence, including negative
replays. It supplies neither edges nor numerical intervals to the analyzer.

## Machine provenance and reused proof

The preheader sets a4 to **`0x5009f3a0`** at `0x500659e0/0x500659e4`.
The initial edge and every loop-back path preserve this base. The all-path
value analysis still proves the same constant after following the table's
case edges; a guessed preheader value cannot replace that recheck.

| PC | Machine fact |
| --- | --- |
| `0x50065a2e` | `lbu a5,0(a3)` reads the loop's index byte |
| `0x50065a32` | `slli a5,a5,2` forms the scaled index |
| `0x50065a34/0x50065a38` | A separate load through s9 tests per-index state |
| `0x50065a3a` | `bnez a0` skips dispatch; its fallthrough alone enters the address addition |
| `0x50065a3c` | `add a5,a5,a4` uses the preserved scaled index and table base |
| `0x50065a3e` | `lw a0,0(a5)` supplies the target register |
| `0x50065a40` | `jalr zero,0(a0)` clears bit zero of the loaded target |

There is no alternative producer between the final load and jump, or between
the address addition and final load. The shift's register survives the
conditional unchanged. Every traversed instruction must be contiguous and
have a unique predecessor on that fallthrough chain; definitions, gaps,
decode conflicts and additional entries prevent the structural proof.

The loop pointer exceeds the existing eight-value alternative limit, so the
converged numerical index and loaded target are unknown. The independent
structural dispatch fact survives that loss. Its old backward slice stopped
at every conditional, including the branch that preserves the scaled index.
The slice now follows a conditional's unique contiguous fallthrough using
the existing CFG predecessor counts and the existing all-path base value.

The compiler table `.LJTI88_0` occupies `0x5009f3a0..0x5009f418`. The existing
`ImageFacts` capture already indexes its label and all 30 relocated words,
checks each relocation against the ELF word, and exposes read-only contents.
`ImageTables` and `RecognizedTables` provide the complete table under their
existing compiler-table contract, even when the numerical index is unknown.
All 30 distinct targets stay inside main. No new index bound is inferred
from the label, a function name, an allocated stack or a disassembly pattern.

The existing finite value join can also prove several possible bases. Every
base must independently prove a complete table; the resulting `KnownJump`
contains their union. A missing source proves no jump. Multiple tables do
not claim a single case-number mapping. RV32 arithmetic remains in the shared
evaluator, including wrapping address additions; table targets apply JALR's
low-bit rule before CFG decoding. Unsupported JALR displacements remain
outside this dispatch recognizer.

`research_following_tables` supplies the targets to the ordinary CFG builder
and re-proves every followed table after rebuilding. A new case edge that
bypasses a definition retracts the proof. This reuses its existing convergence
and invalidation mechanism. `FunctionIndex` navigates the canonical records
for diagnostics and checked stack factories. There is no new ELF traversal,
relocation index, private CFG, closure exception or address-specific resolver.
Other unknown edges and decoding gaps still fail the ordinary closure check.

## Checked propagation and next barrier

| Transition | Proven callee entry sp |
| --- | --- |
| Stack installer to `runtime_main` (`0x50074e2e`) | `0x500dba40` |
| `runtime_main` call `0x50074e32` to main | `0x500dba40` |
| Main call `0x50069490` to CPU0 executor `0x50019a18`, relative sp −2384 | `0x500db0f0` |
| Executor task-poll call `0x50019c1c` to rx-poll `0x5006bb8a`, relative sp −80 | **`0x500db0a0`**, with `ExecutorInvariant` |

The CPU0 path reaches rx-poll. The next barrier is the all-invocation join:
the IRQ poll call at `0x2f003490` is also applicable, and its numerical entry
context remains unknown. The checked merge therefore returns `None`.
Numerical trap-arrival sp/mscratch values and their nonwrapping propagation
through nesting remain unproved. Storage capacities and partial stack budgets
do not supply those values.

The actual constructor call, now `0x5006c66e`, still returns
**`Err(DestinationBounds)`**. The validator does not omit the IRQ invocation
to admit a CPU0-only application. Destination bounds and source preservation
remain separate obligations; the conditional word certificate still does not
establish publisher equality.

Installed contexts still prove only the live sp. A checked call/tail creates
an Entry context without rebasing saved pre-installation pointers. Existing
checks for current capture, caller bytes, context, live inputs and current
shared CFG closure continue to protect prepared proofs from stale reuse.
Neither these contracts nor product code change.

## Verification

The real split shape, union of proved bases, unknown incoming base, missing
table source, discarded x0 load, newly introduced bypass, decode gap and another unknown edge
have regressions using the real decoder and an in-memory ELF. The evaluator
regression checks positive and negative RV32 wrap plus JALR bit-zero clearing
for every finite alternative. The existing Installed/Entry and stale-code,
context and image tests remain enabled.

Tests pass for 52 affected host packages, foundations and consumers (1390
tests, including 235 stack tests and 37 common analysis tests). All-target
clippy under the workspace warning policy and formatting pass; both w0/w1
builds pass. Two independent captures have identical streams and proof JSON.
The positive replay passes; all eight injected bad facts are rejected.

[comparison.json](comparison.json) compares all **14 machine files**, all
**eight complete reports**, and all **four full callback audits** against
clean main in the same checkout. All machine files and callback audits are
byte-identical. All report semantics are identical; only the resource row
changes in the four runtime/bootstrap stack reports. Bounds, unresolved sites,
paths, coverage and assumptions are unchanged. The general stack sweep already
covers the table's local case bodies; the new result closes the canonical CFG
used by numerical stack-context transport. Additional dispatch recognition
and table rechecks increase the charged work. Temporary source words are
released after copying targets; peak memory also depends on the rebuilt graph
and retained proof records.

## Analysis output changes

Only these resource rows change in the eight reports; the comparison retains
their full diffs and every audit diff, without an allowlist.

| Image | Work units before → after | Peak bytes before → after |
| --- | ---: | ---: |
| w0 runtime | 78626291 → 79412338 | 158734474 → 158732258 |
| w1 runtime | 100225973 → 101080078 | 160309569 → 160304793 |
| w0 bootstrap | 15818087 → 16502021 | 16538739 → 19034910 |
| w1 bootstrap | 15817451 → 16499657 | 16653435 → 19149617 |

Reproduce from this checkout, capturing clean main's bundles and diagnostic
executables before changing the analyzer, as in the shared
[capture instructions](../callback-address-audit.md#reproduction):

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/indirect-cfg/check.py \
  target/fw/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-indirect-cfg/validator
python3 tools/analysis/riscv-stack/validation/indirect-cfg/compare.py \
  /tmp/oer-indirect-cfg/before /tmp/oer-indirect-cfg/after \
  /tmp/oer-indirect-cfg/comparison.json
```

The capture logs and full streams for this validation are retained under
`/home/ermacv/.cache/oer-indirect-cfg/` (also linked from `/tmp/oer-indirect-cfg`).
