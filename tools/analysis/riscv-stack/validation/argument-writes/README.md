# Call argument and field-write provenance on w1

Baseline: fresh main `c086c4b9`, including #175 and #176. Both w0/w1 bundles
were built before and after in the same checkout. Captures, full diagnostic
streams and check logs are under `/tmp/oer-argument-store/`. The checked-in
[comparison](comparison.json) contains all 14 machine-file hashes and all eight
report comparisons. [real-transition.json](real-transition.json) records the
actual ABI inputs, parameter-relative write and retained barriers.

## Supported contract

`oer-analysis-riscv-analysis::writes` consumes a canonical `FunctionIndex`:

- A destination/value can be an integer ABI argument word plus a constant,
  an exact linked RV32 word/address, an entry-relative stack address scoped to
  the function, or a value captured by a field load through a proven base and
  constant offset. `Field` retains its function, **load PC**, width, signedness
  and any subsequent constant displacement. It never means entry memory.
- General arithmetic uses `IntegerOp::evaluate`; symbolic recognition is limited
  to addition/subtraction by constants and the full-word identity mask. There is
  no instruction evaluator, private CFG, new ELF reader or new DWARF walk.
  Other expressions can retain an opaque function-local identity; their memory
  effects remain unknown. Symbols/sections that have not become linked exact
  addresses, nested field contents and unsupported forms stay opaque/unknown.
- Stores retain width and occurrence. `Must` is proved only for a complete
  straight-line function ending in a return. Branches/calls conservatively yield
  `May`, which retains the no-write path. Neither form is a final-memory value.
  Every observed overlapping store is retained independently. Calls, unknown
  stores, unsupported effects and atomic operations are explicit barriers.
  An open local graph discards value/address provenance.
- `call_writes` uses the caller's `Transfer` and `CallInputs` facts and explicit
  `CallAbi::RiscvInteger` word registers a0..a7. It unions all recorded callee
  alternatives, conservatively marking multi-callee events `May`. A missing
  target, summary, argument, ABI or caller CFG proof retains unknown effects.
  `CallWrite.source` preserves the parameter-relative proof alongside its
  substituted result. Unknown arguments do not become exact object addresses.
- `ResearchOptions::symbolic_spills` opts the existing value engine's bounded
  **eight entry-stack words** into captured symbolic values. Only matching
  4-byte store/load pairs forward a word. The existing overlap invalidation,
  all-incoming-path intersection join, unknown alias-write invalidation and call
  clobbering remain in force. A fixed projection onto lower stack addresses may
  forget an older word to admit a spill; it never proves non-aliasing. No field
  contents are cached. The default research/callback/publication profile retains
  its previous exact-word behavior.

The shared structural decoder was moved unchanged from `task_spawns` into
`riscv-lift`: structurally known non-control instructions with unsupported
integer effects continue with killed state and a semantic barrier. The exact
local-graph closure predicate is shared with task publication. Its specialized
`Receiver` substitution and publication-dominance contract remain separate;
they are not equivalent to time-scoped field snapshots. `inspect_function`
reuses the stack pipeline's sweep, table proofs, common table-following driver
and supplied `CallBehavior` (the diagnostic uses the existing DWARF facts).

## Actual boundary and IRQ pointer write

`Arena::bind`, `IrqStorage::new`/`split`, `WifiIrqLineParts::install` and
`HandlerCell::install` are inlined on this ELF. There is no standalone install
callee to summarize. The surviving call is:

| Role | Address / actual instruction |
| --- | --- |
| Caller | `rx_task`'s `TaskStorage::poll`, entry `0x5006e4fc` |
| Future argument | `0x5006e648: lw a0, 0(a1)` |
| Context argument | `0x5006e64a: addi a1, sp, 8` |
| Call upper | `0x5006e64c: auipc ra, 0xfffdc` |
| Call | `0x5006e650: jalr ra, 0x428(ra)` |
| Callee | `receive` future's poll body, entry `0x5004aa74` |
| Captured IRQ storage pointer | `0x5004aaf2: lw t1, 0xe4(s3)`; s3 is entry a0 |
| Write into the input object | `0x5004ab56: sw t1, 0xd0(s3)` |

The new reusable summary proves:

```
caller a0 -> callee argument word 0
  -> read4(argument0 + 0xe4) at 0x5004aaf2
  -> may-store4(argument0 + 0xd0) at 0x5004ab56
```

DWARF labels these fields as `future.parts.irq.storage` and
`future.irq.storage`. The actual layout is: receive future DIE `0x8a96a`,
`Unresumed.parts` at `0xd8`, `WifiRxParts` DIE `0x89ca2` with `irq` at `0x0c`,
and `Suspend0.irq` at `0xd0`. These are diagnostic labels from actual debug
layout, not Rust source field order or summary selection rules. The pointer
is to the `IrqStorage` shared by the arena's consumer/publisher; its exact
address and its signal field are not inferred.

The real `CallInputs` fact leaves caller **a0 unknown** and proves a1 is
caller-entry-sp minus 344. Therefore application retains the parameter-relative
source and an unknown concrete destination/base. It establishes the field/write
relationship across this actual ABI boundary; it cannot bind the receiving
future or handler to one exact allocation.

## Spill evidence and the limit on the requested pointer chain

The publisher pointer has a second precise, limited witness:

```
read4(argument0 + 0x104) at 0x5004ab02   // parts.line.mac
  -> store4(callee-entry-sp - 600) at 0x5004af9e
  -> UNKNOWN CALL at 0x5004afd0
  -> reload at 0x5004bdf0 remains unknown
```

`parts.line` is at `0xd8 + 0x2c`, and `WifiIrqLineParts.mac` is at offset 0
(DIE `0xa69ae`). The unknown call is a real memory barrier, even if a register
or source-level ownership relationship appears to survive it.

The new spill profile **does** prove a complete real field/spill/reload/store
sequence in this same callee:

| Stage | Machine fact |
| --- | --- |
| Read snapshot | `0x5004acc4: lw a1, 0x154(s3)` |
| Spill | `0x5004acda: sw a1, 0x1d0(sp)` = entry-sp minus 384 |
| Reload | `0x5004ad06: lw a3, 0x1d0(sp)` retains the original load PC |
| Store | `0x5004ad0a: sw a3, 0x90(a1)` retains that captured value |

The intermediate stores are exact disjoint stack words, not assumed-disjoint
object writes. The store is conditional; its destination object is unknown.
**This word belongs to a `WifiRxExit` payload on the inlined park/report path;
it is not a proved pointer and is not presented as the MAC publisher.** Thus the
new pointer fact above reaches an IRQ storage field, and the complete real
spill witness demonstrates the mechanism on a word. A complete *pointer*
field/spill/reload/object-store witness on the selected publisher/install path
is still blocked. The scope extension does not justify crossing that call.

At `0x5004be26`, the inlined `HandlerCell::install` pointer store remains
`May` with unknown source/destination. The CAS result is checked at
`0x5004be24`; its other path bypasses the store. The release fence at
`0x5004be28` and state-byte write at `0x5004be2e` do not provide a missing
pointer or a publication-order proof for later handler reads.

| Real fact | Before | After |
| --- | --- | --- |
| `0x5006e650` call | raw transfer/inputs | same actual target plus instantiated write events |
| IRQ pointer `0x5004aaf2 -> 0x5004ab56` | separate symbolic load/store facts | parameter-relative captured-field -> object-field summary |
| Publisher `0x5004ab02 -> 0x5004af9e` | separate load/spill facts | captured pointer -> exact entry-stack slot; call barrier retained |
| Word reload/store `0x5004ad06 -> 0x5004ad0a` | unknown value with old spill profile | snapshot from load `0x5004acc4`, width 4, `May` |
| Install `0x5004be26` | unknown pointer, conditional | unchanged unknown pointer, conditional |
| Later `IrqSignal.pend` | type reaches call; 362 candidates | unchanged; no exact MAC_SIGNAL/handler substitution |

A store event does not prove a subsequent read. That needs every possible write,
initial state, aliases and publication ordering. This PR does not supply those
proofs and does not replace the handler's unknown object with `MAC_SIGNAL`.

## Machines, reports, candidates and checks

- All **14 machine files are byte-identical** to freshly built main: runtime
  ELF/bin, bootstrap ELF, application, bootloader, partitions and OTA data for
  each app.
- All **eight reports are byte-identical**, including resource counters:
  placement, runtime stack, interrupt stack and bootstrap stack for each app.
  There are no report changes to attribute to semantic improvements or counters.
  Symbolic spill capture is an explicit summary profile outside the firmware gate.
- All **four full callback-audit TSVs are byte-identical**, including every
  address-use classification and per-site candidate set. Global/local site
  counts and candidate edges are unchanged. Selected `pend` sites retain 362
  candidates. Existing IRQ validation artifacts are unchanged.
- Tests/clippy pass for **43 host packages**: the three changed analysis layers,
  their reverse dependencies, the common ELF/RV32 foundations and dependency-layer
  tests, with `oer-hil-lab/test-support` and serial tests. Stack has 166 tests
  (154 previous plus 12 new); common analysis has 33. New tests include distinct
  callers, offsets, no-write paths, callee unions/missing targets, unknown ABI and
  arguments, closed caller/callee graphs, overlapping writes, atomic barriers,
  captured field reads, overwritten fields, overlapping/narrow spills, unknown
  aliases/calls, optional profiles and x0. Cancellation/admission releases the
  summary's scratch. Formatting and diff checks pass.
- No manifests, lockfile or dependency edges changed. Dependency-layer tests
  pass; the summary uses existing model, index, ledger and arithmetic contracts.

## Reproduction

Build and save fresh bundles and callback audits as in the
[shared instructions](../callback-address-audit.md#reproduction), then compare:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack \
  --example callback-audit --example function-records --example call-writes
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-argument-store/before /tmp/oer-argument-store/after \
  --output /tmp/oer-argument-store/comparison.json
python3 tools/analysis/riscv-stack/validation/argument-writes/explain.py \
  /tmp/oer-argument-store/after/w1/runtime.elf /tmp/oer-argument-store/explained
```

`explain.py` compares the same summary consumer/ELF with symbolic spills disabled
and enabled; it verifies the complete word witness, the IRQ storage pointer write,
the publisher's blocking call and the unknown conditional install. Its addresses
only select/check this pinned diagnostic fixture. Full before/after streams stay
outside the repository. For layout inspection with the existing DWARF tool:

```sh
llvm-dwarfdump --debug-info=0x8a96a --show-children /tmp/oer-argument-store/after/w1/runtime.elf
llvm-dwarfdump --debug-info=0x89ca2 --show-children /tmp/oer-argument-store/after/w1/runtime.elf
llvm-dwarfdump --debug-info=0xa69ae --show-children /tmp/oer-argument-store/after/w1/runtime.elf
```
