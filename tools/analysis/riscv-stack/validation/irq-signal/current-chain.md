# Current w1 doorbell to IrqSignal.pend

Current machine evidence is based on main `73042ea1` (#207 and #208).
See the [stack-context report](stack-context.md): the constructor copy is now
`0x5006c62a`; its actual application still returns `DestinationBounds`.
The earlier investigation below is historical; the regenerated JSON carries
the current PCs, machine setup, both execution contexts and exact gaps.

The machine chain below is the #199 investigation. The subsequent
[one-word copy report](word-copy.md) documents the new conditional certificate
and the numerical bound that still prevents its application. The regenerated
JSON contains both the chain and this follow-up's checked evidence.

Fresh base: main **`694e0c76`**, including #195 and #194. Both unconfigured
w0/w1 images were rebuilt in this checkout before and after the BranchTest
refactor. [The comparison](../short-copy/branch-test-comparison.json) confirms
all **14 machine files, eight complete reports and four complete callback
audits are byte-identical**, including resource counters. Tests and all-target
clippy pass for the 46 affected host packages and consumers.

[current-chain.json](current-chain.json) records input SHA-256 values, physical
function owners, full inline contexts, canonical expressions, memory accesses,
write events, actual stack slots, target-producer facts and callback-field
evidence. Full streams and build/check logs are retained in
`/tmp/oer-irq-chain/`. Addresses and names below select this validation fixture;
they supply no production proof rules.

“Proved” below means the analyzer proves the stated computation or structural
fact. A store event can have `May` occurrence; it does not establish a memory
postcondition, execution on every invocation, or equality at a later load.

## IRQ root and the callback's actual load chain

The existing `interrupt_table` reader validates the immutable slice at
`0x500a6054`, entries at `0x500a51e0`, and each relocated handler word.
Entry 2's handler word **`0x500a5200`** holds **`0x2f00428e`**, source 68,
**hart 0 / level 3**. The firmware interrupt report independently includes the
selected unresolved callback under that same hart and level.

| Machine PC / storage | Physical owner | Existing fact | Status |
| --- | --- | --- | --- |
| `0x2f004292` | `FROM_CPU_INTR3`, entry `0x2f00428e` | Direct tail to `doorbell`, `0x2f0035dc` | Proved |
| `0x2f00369e` | `oer_firmware_apps_w1::irq::doorbell` | `lw s1,1216(a0)`; expression 47 loads word **`0x2f0174c0`**, `RX_LINE` | Proved load address; **unknown loaded object identity** |
| `0x2f0036fe` | `doorbell` | `lw a0,0(s1)`; expression 61 loads through 47 | Proved dependency; unknown concrete publisher |
| `0x2f003706` | `doorbell` | expression 64 loads through 61 | Proved dependency; unknown concrete IrqStorage |
| `0x2f003708` | `doorbell` | expression 65 loads through 64 | Proved dependency; unknown concrete IrqSignal |
| `0x2f003712` | `doorbell` | Atomic write through expression 65 + 12 | Proved event; existing base-transport barrier |
| `0x2f00371c` | `doorbell` | expression 68 loads word through 65, the `pend` field by inline/DWARF context | Proved dependency and callback field type; unknown word contents |
| **`0x2f00371e`** | `doorbell` | `jalr ra,0(a1)`; a0 = expression 65, a1 = expression 68 | Proved call inputs; **exact target unknown** |

These expression IDs are local to the saved handler stream. The validator
checks each load's address, width, defining expression and actual instruction,
and reachability over the **emitted common CFG**. It never reconstructs a CFG
from disassembly.

The normal target-producer analysis already proves `0x2f00371c` is the
**exclusive reaching load**. The shared callback audit finds the function-pointer
field with subroutine DIE **2461413**, using the existing pointer witness at
`0x2f00371a` (and the retained witness at `0x2f003716`). The base suffix stops
at `0x2f003712`, `MemoryWrite`. All **362** audited typed candidates remain,
including `pend_signal::<2>`. Neither the field type nor that candidate proves
this runtime call targets that function. The experimental typed resolver remains
outside the firmware gate.

## Trace backward to initialization

The ELF initializer for **`MAC_SIGNAL`**, data owner
`oer_firmware_apps_w1::irq::MAC_SIGNAL`, occupies
`0x2f007004..0x2f007028`. `ImageFacts::word(0x2f007004)` and address-use **796**
prove its initial first word is **`0x2f003222`**,
`oer_espressif_executor::executor::pend_signal::<2>`. The existing audit marks
that storage typed (subroutine DIE **702463**) and admits it for this callback.
`ImageFacts::read_only_word` returns **None** at this writable address. This is
an **ELF initializer fact**, not proof of its value at the selected call.

The source binds this signal to `InterruptExecutor::<2>::SIGNALS`, the P2
executor. The machine's initializer points to its `pend_signal::<2>` instance.
The outstanding object-identity proof prevents claiming the selected call
actually invokes it; no complete IRQ stack bound follows.

The physical owner of every constructor PC below is
**`<embassy_executor::raw::TaskStorage<oer_firmware_apps_w1::irq::__rx_task_task::__rx_task_task_inner_function::{closure#0}>>::poll`**,
entry **`0x5006bbca`**. Let **E** mean that function's symbolic entry a0.
Inline contexts locate `Arena::bind`, `MaybeUninit::write`, `receive`,
`WifiIrqLineParts::install` and `HandlerCell::install`; they label the
operations, without supplying missing machine values.

| Machine PC | Existing canonical fact in rx-poll | Status |
| --- | --- | --- |
| `0x5006c590` | Store `0x2f007004` to `0x2f017540`, the arena's IrqStorage signal word | Proved address and value of this store |
| `0x5006c592` | Store `0x2f017540` to **`0x2f017544`**, the publisher's storage word | Proved address and value of this store |
| `0x5006c5ee` | Store publisher **`0x2f017544`** to **E + `0x580`** | Proved affine address/value; `May`, `ClosedGraph` write event |
| `0x5006c602` | Store line slot **`0x2f017800`** to **E + `0x590`** | Proved affine address/value; `May`, `ClosedGraph` |
| `0x5006c606` | Store publisher `0x2f017544` to E + `0x594` | Proved second capture; `May`, `ClosedGraph` |
| `0x5006c6ae` | Call `0x2f8004c4`, a0 = entry-sp − 280, a1 = E + `0x554`, a2 = **68** | Proved call inputs; **copied contents unknown** |
| `0x5006c6c0` | Same target, a0 = E + `0x38`, a1 = entry-sp − 496, a2 = **284** | Proved call inputs; **copied contents unknown** |
| `0x5006c6f4` | Store **`RX_LINE = 0x2f0174c0`** to **E + `0x15c`** | Proved affine address/value; `May`, `ClosedGraph` |
| `0x5006bcc4` | Same target, a0 = E + `0x2b8`, a1 = E + `0x38`, a2 = **640** | Proved call inputs; **copied contents unknown** |
| `0x5006bcd0`, `0x5006bcd8` | Store pointers to the receive future and its wrapper into E-relative fields | Proved local expressions; later object-field loads are not forwarded |
| `0x5006bd20` | `lw s9,0(a0)`; canonical memory address is `Unknown` | **Unknown receive-future base** across initial/resumed paths |
| `0x5006bd70`, `0x5006bd88`, `0x5006bd94` | Load publisher, line slot and cell through s9 + `0x104`, `0x114`, `0x124`; canonical addresses are `Unknown` | **Unknown values before any selected spill preservation** |
| `0x5006d1c4` | `sw a2,0(a5)`, line's first publisher field | Write summary has **unknown address and unknown value** |
| `0x5006d1f6` | `sw a5,0(s1)`, in `HandlerCell::install` | Write summary has **unknown destination cell and unknown published pointer** |

This table follows construction order, not numerical PC order: the emitted CFG
jumps back to `0x5006bcb2` after construction. All selected instructions are
reachable in that graph; reachability is not proof of memory contents.

The exact **current** entry-stack coordinates used before publication are:

| Value used by installation | Actual definition | Actual reload | Existing fact |
| --- | --- | --- | --- |
| Publisher, slot **−532** | `0x5006bd74: sw a0,44(sp)` | `0x5006d1c2: lw a2,44(sp)` | Definition stores `Unknown`; reload summary remains `Unknown` |
| Line object, slot **−556** | `0x5006c368: sw ra,20(sp)` | `0x5006d1c0: lw a5,20(sp)` | Definition stores `Unknown`; reload summary remains `Unknown` |
| Destination cell, slot **−524** | `0x5006bd98: sw a0,52(sp)` | `0x5006d1aa: lw s1,52(sp)` | Definition stores `Unknown`; reload summary remains `Unknown` |

The frame is 576 bytes. The JSON retains **every same-slot access**, including
other definitions on other paths. Those lists are not a new reaching-definition
or alias proof. Preserving these slots across a call alone cannot create the
missing pointer values: the selected definitions already lack them.

There is a useful **conditional layout calculation**, explicitly unproved as
a machine memory relation. If the three calls implement the expected copies,
the captured publisher at E + `0x580` would pass through entry-sp − 236,
E + `0x13c`, then E + `0x3bc` = `(E + 0x2b8) + 0x104`. The line slot at
E + `0x590` would pass through entry-sp − 220, E + `0x14c`, then the future's
`0x114` field. The cell at E + `0x15c` would reach its `0x124` field through
the 640-byte call. These calculations label the missing connections; the
validator never feeds them into the analyzer as facts.

In [the source](../../../../../firmware/apps/w1/src/irq.rs),
[`Arena::bind`](../../../../../chips/esp32s31/ieee80211/driver/src/rx/arena.rs)
returns these parts, `receive` installs the line into
`RX_LINE`, and the handler reads that same object and publisher. **The complete
constructor-to-handler equality is source-only and unconfirmed by machine
analysis.** In particular, `0x5006d1f6` is not a proved store to `0x2f0174c0`.

## Is the #195 20-byte memcpy the barrier here?

**It is not the demonstrated pointer-preservation barrier on this path.**
The fresh call is still **`0x5004db66` → `0x2f8004c4` → `0x2f80d260`**, with
proven entry a2 = 20. Its physical owner is
**`oer_firmware_apps_w1::run::{closure#0}`**, entry **`0x5004d7c8`**, with a
**256-byte** frame. Both canonical memory records and the existing
`stack-call-writes` before/after reanalysis contain **no spill or reload at
−600**. The selected constructor has a different physical owner, different
actual slots, and the concrete 68/284/640-byte calls above. Its pointer values
are already unknown at their spill definitions.

The validator reruns the real #195 specialization: five retained write events
from `0x2f80d27a`, `0x2f80d2c8` (three) and `0x2f80d338`; nine bulk stores are
excluded. The surviving unknown stores still do **not** establish general
noninterference. `context-preserves false` for a requested −600 coordinate
does not establish that a relevant live pointer ever occupied it.

For comparison it also applies the current machinery to the **actual first
constructor copy**, `0x5006c6ae`, length 68. All fourteen write effects remain;
the footprint is incomplete and no copied-word postcondition is available.
The queried **−236** is the conditional destination coordinate of the publisher
word, **not an observed captured spill**.

## First missing fact and the next general change

Starting at the selected IRQ root, the first unproved object transition is
**`0x2f00369e`: `*(RX_LINE)` → the initialized WifiIrqLine object**. Missing:
an all-path mutable-memory reaching definition linking this load to an actual
publication of that object. The candidate publishing instruction
**`0x5006d1f6`** currently proves neither its destination equals `RX_LINE` nor
its value equals the initialized line slot.

The existing common **value analysis / FunctionIndex** must supply object
addresses and loaded values. **FunctionWrites / CallWrites** already retain
write events, provenance, barriers and call substitutions; **StackCalls** only
preserves an already captured stack word under complete noninterference.
Target-producer transport and callback-field discovery already succeed here.
Write summaries explicitly are **not memory postconditions**; neither their
presence nor static callback initialization closes this gap.

A concrete next change is a bounded common **memory-word provenance** domain
for exact affine object fields. Its first constructor checkpoint should be a
machine-proved copy-word effect at **`0x5006c6ae`**, transporting the word at
E + `0x580` to entry-sp − 236, with exact source/destination/length premises.
Use the existing CFG and evaluator to prove every possible writer and byte
coverage, then compose through the existing transparent tail and call
substitution. Preserve load-event identity, width, occurrence and ordering;
unknown aliases, uncovered bytes, unequal incoming values, unsupported paths
and unproved calls invalidate the result. No function name or ROM contract
may assert copy semantics. Merely adding more unreachable-store exclusions
cannot supply this copied value.

That first general extension would address a concrete constructor prerequisite;
284/640-byte transports, resumed-future field forwarding and cross-function
publication still require their own proofs before the handler's first barrier
can close. This PR implements **none** of those production mechanisms and adds
no trusted contracts.

## Reproduce

Run from the repository root, using the pinned toolchain. Keep the successful
fresh w1 bundle's `interrupt-stack.txt` beside its ELF. To reproduce the
before/after comparison, capture w0/w1 bundles and four complete audits using
the [shared instructions](../callback-address-audit.md#reproduction), before
and after the BranchTest commit, in the same checkout path.

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/irq-signal/explain.py current-chain \
  target/fw/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-irq-chain/reproduced
cargo test --locked -p oer-analysis-riscv-stack -p oer-test-layers
cargo clippy --locked --all-targets -p oer-analysis-riscv-stack
cargo fmt --all -- --check
```

The extended **existing validator** invokes `callback-audit`, `function-records`
and `stack-call-writes`. `function-records` optionally exposes ImageFacts
symbols/address uses/words, the existing IRQ-table reader using the existing
chip contract, standard inline contexts, symbolic spills and FunctionWrites.
Its default fact stream is unchanged. There is no additional ELF/DWARF parser,
instruction interpreter, private CFG, callback resolver or source-derived
fact input. A changed fixture fails its asserted facts instead of carrying
historical slot assumptions forward. ELF, ROM, report, chip contract and ROM
summary hashes, diagnostic binary hashes and full stream hashes are recorded.

Two independent fresh captures produce identical chain JSON. The manual
[validator checks](current-chain-checks.json) also reject injected changes to
the pend-load width and to the actual line-slot spill's stored value. The
committed evidence comes from the unmodified diagnostics. After the diagnostic
extension, 206 stack tests and seven dependency-layer tests pass; all-target
clippy and formatting pass with the workspace's existing warning policy.
