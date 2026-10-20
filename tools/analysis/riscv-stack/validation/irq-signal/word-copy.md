# One-word constructor copy: conditional certificate and remaining bound

Base: main **`ab2d9f96`**, including #199 through #205. This follow-up adds a bounded common
word domain and an independently checked normal-return word postcondition.
The real call has a **conditional** four-byte certificate through the existing
trampoline and actual argument substitution. **Its application is rejected**:
the current facts do not prove the required numerical destination bound.
The unconditional real-call acceptance criterion is therefore still open.
No firmware, dependency manifest, ROM summary or trusted contract changes.

[current-chain.json](current-chain.json) records the fresh inputs and hashes,
the conditional caller instance, both body/trampoline certificates, every
memory event on the admitted return path, and a final-writer check for each
byte. [word-copy-comparison.json](word-copy-comparison.json) records all machine,
report and callback comparisons. Full bundles, traces and checks are retained
under `/tmp/oer-memory-word/`. The initial #199-base capture is retained
separately under `/tmp/oer-memory-word-base199/`; the selected call, frame
coordinates and ROM path are unchanged on the rebased image.

## Reused mechanisms

- `values::State` retains the existing eight-word capacity, overlap handling,
  fixed projection, worklist and joins. The opt-in `symbolic_spills` profile
  additionally admits normalized entry-register-plus-offset and exact numerical
  addresses. The same evaluator emits the loads, stores and register values.
- Only the **same base** with disjoint RV32 byte coordinates establishes
  nonoverlap. Different entry registers, stack versus an entry register, unknown
  aliases and unsupported expressions invalidate potentially affected words.
  Conflicting or absent incoming memory values are dropped. Calls discard them
  unless an existing checked preservation or new checked postcondition applies.
- `FunctionIndex`, the prepared common CFG/operations, `IntegerOp::evaluate`
  and `BranchTest::evaluate` remain the source of machine semantics. A bounded
  path driver reuses that evaluator to check one queried word on every reachable
  normal-return path. It does not decode instructions or build another CFG.
- Existing tail transparency, `Values`, `substitute`, `call_context`, generic
  `FunctionWrites`/`CallWrites` and `StackCalls` perform transport and live-input
  revalidation. Generic effects and barriers are never removed by a word fact.

The optional path driver follows at most 64 paths and 4096 instructions per
path, and admits a query span at most 4096 bytes. Its DAG growth and retained
event traces are admitted through the existing working-memory budget. Limits,
unsupported instructions, opaque calls, incomplete graphs, unproved branches
or loops that exceed the bound produce no word certificate or the existing
budget/cancellation error. There is no length-68, function-name or ROM-address
rule in production code.

## Contract of the new fact

`WordPostcondition` is separate from write events and has private construction.
Under its entry-argument, alignment and optional numerical-bound premises, every
modeled normal return has one complete four-byte word whose last writer stored
the value captured by a specified four-byte load. The certificate retains:

- Original function owner, load/store PCs, instruction ordinals and width.
- Destination and source-load addresses, with their scope explicit before and
  after call substitution; the transparent-tail witness preserves ownership.
- Every memory access and return on every admitted return path. A partial,
  unknown or possibly overlapping later writer invalidates the word unless a
  subsequent complete proved store establishes it again.
- A **separate** `source_unchanged_before_load` proof. Without it, load-time
  provenance says nothing about the source word's value at call entry.

`WordInstance` exposes a conditional relation and any missing premise after
substitution. Only `into_checked` yields the application accepted by forwarding.
`call_word` performs those checks; `StackCalls` rechecks the actual target,
caller/site, ABI and all required live normalized arguments before every reuse.
Only immutable canonical facts construct a cache entry. Expression identities
from a previous capture are not used as live argument identities.

When the source was proved unchanged before its load, forwarding may reuse an
independently captured source word. Otherwise `Expression::CallWord` describes
the **callee load-time** value, with the original load/store owner and order;
its source-address DAG belongs to the containing caller index. Blobray register
classification and the CLI field walker keep this expression opaque. It is
never silently reclassified as a fresh source-field read or an entry value.

The queried four-byte destination must be aligned. For an entry-stack-relative
destination, alignment follows from the explicitly selected standard RISC-V
ABI and a multiple-of-four offset; that ABI specifies a 16-byte entry-stack
alignment. It provides **no numerical stack interval**.
[RISC-V psABI, integer calling convention](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention).

## Real machine chain

Physical caller: rx-task `TaskStorage::poll`, entry **`0x5006bbca`**.
E denotes its symbolic entry a0. Actual call **`0x5006c6ae`** has:

| Fact | Existing machine input / derived conditional relation |
| --- | --- |
| Target | `0x2f8004c4`; existing single-jump transparency to `0x2f80d260` |
| Destination | entry-sp − 280 |
| Source | E + `0x554` |
| Entry length | 68 |
| Queried source word | source + 44 = **E + `0x580`** |
| Queried destination | destination + 44 = **entry-sp − 236** |
| Source load | **`0x2f80d332`**, ordinal **51**, `lw a7,0(a6)`, width 4 |
| Last destination store | **`0x2f80d338`**, ordinal **53**, `sw a7,0(a2)`, width 4 |
| Normal return | **`0x2f80d2be`**, after the remaining tail stores |

The certificate covers destination offsets **44,45,46,47** from the matching
source offsets. All four have that same final store; later stores on the
certified aligned, bounded path do not overlap them. The event trace contains
17 loads, 17 stores and the return. The validator checks all four final writers
and checks body/trampoline event equality. Actual substitution is produced by
the existing Rust machinery, not inferred solely from this table.

The ordinary call still retains all **14 generic write events** and their
uncertainties. A certificate for this word is not a complete memcpy contract.
For this certificate `source_unchanged_before_load` is **false**: earlier
destination writes may alias the source through the two distinct symbolic
bases. No equality with the publisher is asserted, even if a caller-side store
of that publisher was observed.

## Why actual application remains unknown

The proof requires **D mod 4 = 0** and, for the unsigned pointer comparisons,
**`0 <= D <= 0xffffffbb`**, where D is the destination. The first condition is
checked for `entry-sp − 280` under the selected ABI. The second is not supplied
by `CallInputs`, `EntryStack`, relative frame size or the current `FunctionIndex`.
Accordingly both diagnostic application and the forwarding factory return
**`Err(DestinationBounds)`**. No destination memory state is installed for this
actual call, and the constructor-to-IRQ chain remains unresolved.

The bound is material. For the model input **D = `0xffffffc0`**, length 68:

```text
end                    = (D + 68) & !3 = 4
pointer after bulk 36  = D + 36        = 0xffffffe4
unsigned(pointer < end)               = false
```

The machine can return at `0x2f80d2be` before writing D + 44. These numerical
inputs are not excluded by a purely relative stack coordinate. This is a
counterexample in the current instruction/memory model, not a claim that the
actual firmware stack lives at this address. A valid concrete stack interval
would exclude it, but neither a historical spill nor the ABI alignment proves
that interval. General affine words use modulo byte comparisons and need no
invented nonwrapping premise; the unsigned loop comparison specifically does.

The next necessary fact is a **checked numerical address interval for this
call's stack-derived destination**, obtained from machine context and the
existing stack-layout checks. A concrete follow-up would propagate checked root
stack placement through validated startup SP setup and proven stack/call deltas,
publish the resulting entry-stack interval in canonical facts, and revalidate
it during call substitution and cache reuse. Unknown transitions must retain
unknown bounds. This work does not introduce a trusted interval to bypass that
missing proof. Source preservation/nonalias, the 284/640-byte
copies, resumed-future fields and `RX_LINE` publication remain separate
obligations. The first handler-side barrier remains `0x2f00369e`.

## Validation and reproduction

The affected 46 host packages and consumers pass tests and all-target clippy
under the workspace warning policy. Formatting and diff checks pass. Ten new
end-to-end groups exercise store/call/load; callee load-time versus entry
contents; conditional stores and conflicting returns; partial overlap and
unknown aliases; later caller writes and joins; changed live arguments,
length/target/ABI; width and order changes; wrapping input and missing bounds;
every cancellation checkpoint in proof, tail-trace composition, preparation
and caller evaluation; and refused admission budgets with complete release.

Both w0/w1 builds pass. All **14 machine files and four full callback audits
are byte-identical**. All eight reports have identical semantic content;
placement and interrupt reports are byte-identical. Only the `analysis:` row
changes in runtime/bootstrap stack reports. Larger exact-address keys and the
new expression representation increase accounted bytes; strict agreement in
memory joins avoids some set interning/worklist work. No bound, unresolved
site, candidate set or coverage assumption changes.

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/irq-signal/explain.py current-chain \
  target/fw/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-memory-word/reproduced
target/tools/examples/stack-call-writes target/fw/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  chips/esp32s31/platform/rom-functions.toml 5006c6ae -236 --word-offset 44
```

The shared [before/after capture](../callback-address-audit.md#reproduction)
and comparator reproduce the artifact comparison. The current validator hashes
ELF, ROM, report, existing contracts, binaries, validator sources and complete
streams. Two independent captures are identical. All five injected changes
(target-load width, captured slot, source-load width, later overlapping store,
and forged admission without bounds) are rejected; see
[current-chain-checks.json](current-chain-checks.json). Generic ROM/call effects
also match the saved base executable's output exactly. The byte audit checks
emitted facts; it supplies no machine semantics, memory contents, caller bounds
or production proof premises.
