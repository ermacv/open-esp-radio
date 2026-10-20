# RV32 stack bounds

The opt-in [stack-address context](validation/irq-signal/stack-context.md)
connects canonical machine setup and call inputs to numerical word-proof
premises. It is scoped to an image capture and invocation; placement or a
partial stack budget alone never establishes a current-SP interval.

`oer-analysis-riscv-stack` computes worst-case stack bounds of a static RV32 image from
its machine code, over the program model of
[`oer-analysis-riscv-program`](../riscv-program/README.md),
[`oer-analysis-riscv-analysis`](../riscv-analysis/README.md) and
[`oer-analysis-riscv-lift`](../riscv-lift/README.md). It reads the image through one
`ImageFacts` of `oer-analysis-riscv-program` (functions with their aliases,
executable ranges, read-only words, sized data objects, relocation-visible
address uses, checked address words and `.LJTI*` labels, parsed once) and its frame records, relocations
and DWARF through [`oer-analysis-elf`](../elf/README.md), whose RV32 relocation
table decides which relocation is an address word or a control transfer.

`taken_addresses`, task-spawn ownership and the shared callback audit
read the common address-use index. Their projections preserve distinct
rules: `taken_addresses` excludes direct section targets and clears the
instruction low bit; spawn ownership recognizes source function symbols
only of kind `Text`; typed provenance recognizes any source symbol at a
function entry. Both the callback and task-spawn audits retain arithmetic, odd, interior and
unknown-role uses, which cannot prove an exact typed materialization.
They project onto the joint analysis's numerical function catalog without
interpreting another ELF's section identities. Field-type, executor and
instruction-pattern proofs remain in their consumers. The callback audit is shared by the global and experimental local resolver.

`analyze` works in the caller's `WorkingMemory` and charges the caller's
`RunControl`, as Blobray's analyses do: running out of either fails it with
`ResourceLimited` instead of running on. The queries over an image's facts
(`stack_sizes`, `taken_addresses`, `waker_vtables`, `interrupt_table`,
`vector_table`, `trap_entry`) charge the same control for each relocation,
word, entry or step they read. The caller decides both; the image
checks' budget is `oer_image_checks::stack::budget`. Every structure that
grows with the image is admitted in that memory before it is made: what
the analysis keeps (each function's facts, the DWARF facts and type facts
of `read_debug`, the vtables, taken addresses, frame records and
resolutions) through the caller's `Ledger`, which it holds while it keeps
them, and what one function's sweep and value analysis build through a
ledger released with it. `read_debug` reads the DWARF once, from sections
borrowed from the ELF's bytes; its inline chains are addr2line's over the
same bytes (`Symbolizer::shared`), whose lazily grown caches are library
internals the working memory does not account.

The image pipeline requests `DebugMetadata::TypedLocations` from `read_debug`:
local/parameter locations and global storage types are collected in the same
DIE walk. `audit_function_addresses` prepares instruction aliases once and
audits the existing `ImageFacts::address_uses` observations. Both resolvers
accept this same `FunctionPointerAudit`; neither loads DWARF or rebuilds an
ELF/symbol index. `DebugMetadata::Standard` remains available to callers that
need no local locations; their code stores stay unknown to the audit. The
experimental local resolver requires prepared local locations. No global
cache is used.

Local locations preserve exact half-open PC ranges, location-list gaps and
independent byte-aligned aggregate pieces. Missing or unsupported pieces
advance the object offset; unsupported bit pieces stop subsequent offsets.
A variable's own type takes precedence over its origin/specification chain;
locations themselves are direct attributes. Globals retain their separate
absolute-address/piece rules, signatures retain their own inheritance rule,
and `noreturn` requires agreement of direct attributes on concrete
subprograms. Sharing expression decoding and source-reference selection
does not unify those interpretation policies. Retained facts and temporary
range/piece storage use the caller's memory ledger; per-unit ranges and
per-expression global pieces release their reservations when complete.

Facts of the build come first. An image linked with `--emit-relocs` keeps
its relocations, each checked against the word it describes (a mismatch is an
integrity error): a compiler jump table is the run of relocated words from its
`.LJTI*` label, however the code indexes it, and its entries inside the
function become edges of the value analysis's graph, which then also reaches
the code behind them. Any other table is located as Blobray locates it: the
value analysis records each jump through `table[index]` as a `Dispatch`
record, the shared recognizer (`RecognizedTables` of `oer-analysis-riscv-analysis`)
proves its entries from the image's facts (`ImageTables` of
`oer-analysis-riscv-program`: a labelled table, a sized object or a run the guarding
bounds check counts), and the shared driver (`research_following_tables`)
analyzes the function again following it, while each pass proves again
every table it followed and more; a pass that loses one reports the
function following only the tables the sweep names. The sweep stays for
transfer coverage: a base kept across an early return or in a register set
far before the jump, which a linear sweep forgets, is still located. A jump
followed through a proven table, whose entries all lie in the function, is
no unresolved transfer.

Illegal-instruction traps are exactly `c.unimp` (`0x0000`, with C enabled)
and `unimp` (`0xc0001073`); `ebreak`, `ecall` and other exceptions retain
their existing behavior. The image pipeline derives `TrapSemantics::Terminal`
only when every hart has (a) a checked `trap_entry` that switches to the
interrupt stack before memory access and calls one handler on all paths,
(b) a proven nonreturning handler, using a closed graph without reachable
`Return`, whose exits call or tail-transfer to proven nonreturning functions
or loop forever, and (c) an exception-root `Bound` with no `level_drops`
(trap returns or writes of `mnxti`/`mintstatus`). This proof runs with
`traps = Unknown`, so a handler's own `unimp` cannot justify itself. The
result applies to every function: `Unknown` leaves a gap, while `Terminal`
ends the path with a targetless `Trap` edge. The reports print the result
and any failed condition. The exception path adds no task-stack cost:
the checked entry uses the interrupt stack, and the exception bound is
already included above every hart's interrupt levels.

`analyze` also takes reviewed summaries (`Summary`, parsed from a
`[[function]]` TOML such as `chips/esp32s31/platform/rom-functions.toml`) of
companion functions the machine code alone does not bound: each names its
function's address, which must carry that name. A summary of a function the
image reaches applies: where the analysis bounds the function itself, the
two must agree on frame and calls, and otherwise the summary stands for it.
`Analysis::summaries` lists the ones that applied; a summary no image of the
platform uses is stale, which the platform's audit over all its images fails.

`analyze` takes the image and its companions, such as the chip's ROM ELF
(`esp32s31_rev0_rom.elf` of the pinned `esp-rom-elfs`): a call from one into
another reaches the callee like any other, and no two may place code at the
same address. Every function symbol is a function, and so is a global
untyped symbol in an executable section outside every sized function (an
assembly entry, the ROM's `__call_*` trampolines); a symbol without a size
extends to the next one of its section. Its frame is the compiler's `.stack_sizes`
record, cross-checked with the deepest entry-relative `sp` the value analysis
observes under the psABI's preserved `sp` (a deeper observation is an error);
without a record, the observed depth counts only when the function's
control-flow graph is complete. Its transfers are every direct call and
out-of-function jump of a linear sweep over its whole extent, which also
reaches code behind jump tables the graph does not expand, plus the indirect
transfers whose target the value analysis resolves; an indirect jump whose
targets the analysis finds inside the function is a jump table, not a
transfer. A call or jump through `table[index]`, whose base is a constant or a
register the value analysis knows at the transfer, reaches each entry of the
table when its length is exact: the index's bounds check on the path to the
dispatch (`bltu limit, index` or `bgeu index, limit` against a constant) or
mask (`andi`), or the size of a data object the base starts in a section
neither writable nor executable, against which the language bounds-checks the
index (a table in code loaded into RAM, such as esp-hal's
`__EXTERNAL_INTERRUPTS` handler table, is rewritten at run time, so its file
bytes are no constant); a zero entry is an empty
`Option<fn>` slot, never a target, since a Rust function pointer is not
null. Entries of a jump inside its own
function are already in the sweep. A table of unknown length, such as a
`match` on an enum whose range no check states, stays unresolved: reading
until the first word outside the function could stop short of an entry that
leaves it.

A function's bound is its frame or, if deeper, a callee's bound below the
`sp` of the transfer that reaches it: the analysis's depth at that site, or the
whole frame at a site it did not reach. The bound never underestimates the
code it reaches.

Bounds fail closed. A root has a number only when nothing it reaches is
unresolved; otherwise `Bound::reasons` counts the unresolved sites and
functions by `Reason`, and `Reason::closed_by` names the stage of the stack
analysis that resolves each: indirect calls through a stack slot
(`core::hint::black_box`), through a loaded pointer or another register,
indirect tail calls through a pointer, transfers outside the image (ROM) or into
a function's middle, functions without a frame, recursion, and indirect
sites whose facts found no target. `Bound::partial` is the deepest path over
what is resolved: the bound when nothing is unresolved, otherwise `partial +
?`, neither an upper nor a lower bound, since a hole may reach deeper and a
path may be infeasible. Interrupt contexts (`LevelStack`, `HartStack`) carry
the same `partial`.

Traps run on the hart's interrupt stack. `vector_table` reads a vector
table's entries from the words its relocations name (an empty slot holds
zero), and `trap_entry` checks one entry by executing it symbolically on every
path to the call of its handler: the trap arrives with the interrupted `sp`
and `mscratch`, the other stack (the interrupt stack's top in thread mode, the
task's `sp` inside a trap), and the interrupt stack (SRAM) lies below every
task stack (PSRAM). Each memory access must address the entry's frame below
the lower of the two, which the entry may learn only from an unsigned
comparison of both (a register swap through three `xor`s keeps the values
apart); an access before that, an unknown transfer or different handlers on
different paths fail. `TrapEntry::frame` is the deepest `sp` below the
interrupt stack's position at the handler call: what every nesting level adds
beside its handler's bound.

Facts outside the machine code close indirect sites through
`Analysis::bound_with` and its `Resolutions`, which apply only where the
analysis left a target unresolved. Each site's targets carry the `Fact`s that
gave them; the facts of one site unite, each being a superset of what the
site reaches. An empty set resolves a site only where its fact is complete
for it (`Fact::proves_empty`: the interrupt table, the IPC posts); an empty
set of found facts (waker vtables, field types) is a hole,
`Reason::NoCandidate`, never a site that reaches nothing:

- `interrupt_table` reads an image's interrupt table (the slice an exported
  symbol holds, its pointer word and every non-zero handler word carrying the
  relocation that names it) in the chip's `TableLayout`.
- `Dwarf` reads the chain of functions inlined at an address and the
  qualified type each function returns. `waker_vtables` finds every
  `RawWakerVTable`: four relocated function pointers in data neither writable
  nor executable whose first function returns `core::task::wake::RawWaker`.
  A `&'static RawWakerVTable` points at immutable data, so every vtable a
  `const` or `static` builds is found; one built at run time in writable
  memory (a leaked allocation) is not. `waker_resolutions` sends an
  unresolved call inside `Waker::wake`, `wake_by_ref`, `drop` or `clone` to
  that slot of every vtable. When the inline scope ends before the call,
  every reaching target load in a closed local control-flow graph must
  identify a method and its matching slot. A merge of methods keeps the
  union of their slots; an unknown producer leaves the call unresolved.
- `Analysis::constant_arguments` reads an argument register's exact value at
  every direct call of a function, complete when `taken_addresses` holds no
  relocation that takes the function's address other than a transfer: how
  the ESP32-S31 image reads every function `Ipc::call_function` posts, the
  targets of the IPC dispatch.

`task_poll_resolutions` narrows each executor's poll targets when all uses
of a task's poll address prove its storage initialization and executor
assignment. The executor's publication must dominate the initialization.
Unknown spawn paths keep the task among every executor's candidates, and
the completed-task poll remains a candidate for all executors. These facts
retain the executor invariant assumption described below.

A direct recursive function can be unfolded once when path-sensitive
instruction facts prove that every self-call passes a payload for which
another self-call is unreachable. The terminal invocation still includes
the function's full frame and every nonrecursive callee. Unknown control
flow, unsupported operations, loops and exhausted analysis budgets prevent
the proof; indirect and mutual recursion remain unresolved.

`function_pointer_resolutions` discovers fields through absolute globals:
the sweep's load address, or the value analysis's base register plus the
load displacement. Merged/split global pieces retain their own DWARF rules.
`typed_function_pointer_resolutions` separately discovers fields through
PC-specific pointer/aggregate locations and complete reaching target loads.
These APIs intentionally recognize different sites; neither discovery rule
is extended by the shared audit.

Both use `audit_function_addresses` and exactly one candidate rule. Each
relocated use of a function address retains its original index and either
all proven storage types or a specific `UnknownUse` reason. An absolute word
can prove a unique global function-pointer field. Code materializations use a
small straight-line proof: the absolute HI20/LO12 form or a PC-relative
AUIPC/ADDI pair must reach typed word stores before the result register dies.
PC-relative binding reuses `PreparedReferences::{normalized,pair}` and
`RiscvDecoder::reference` through an adapter over the prepared address uses;
LO12's label is kept for diagnostics while its effective target comes from HI20.
The linked instructions and `LowerPcRelative` semantics must agree, every gap
must preserve the upper register, and no incoming path may bypass the upper or
enter the subsequent lifetime. Unsupported forms and unclosed graphs stay
unknown. An unpaired/unresolved low cannot identify a callback owner and keeps
the joint function catalog as candidates. Function merging unites all aliases'
uses at their shared entry.
Addends retain both their source function and numerical containing function
when different. Facts remain bound to their source ELF; another ELF's DIE
and section offsets cannot establish a storage proof.

A candidate remains when any storage type can match any call-field type,
when any use is unknown, or when its taken address has no storage evidence.
Matching compares parameter/result sizes and, for two Rust-ABI storage
types, the names rustc assigns; a missing signature fact excludes nothing.
A cast before storage therefore participates through its stored type. A
function declaration, a merged symbol's declaration and a trait method's
arity alone prove no pointer provenance. Empty candidate sets still leave
`Fact::FieldType` calls unresolved. Memory recovered through an erased
pointer retains the existing executor-invariant assumption.

The firmware gate continues to use global field discovery. The experimental
local resolver remains available separately; its current address-use coverage
leaves candidate sets too broad for useful bounds. Interprocedural pointer
propagation, constructor summaries and new memory models are later work.

The [callback audit validation](validation/callback-address-audit.md) records
real w0/w1 unknown-use classes, missing facts, before/after results and the
commands to reproduce the diagnostic without function/address allowlists. The
[PC-relative validation](validation/pcrel/README.md) additionally explains every
normalized pair, changed use and changed candidate in the real w0/w1 results.

`interrupt_stacks` bounds each hart's interrupt stack (`Stacks`): one
interrupt per level the hart takes (its table entries' levels and the levels
it always uses), each the worst hardware-vector entry's frame plus its
handler's bound, where the dispatcher's calls through the source table reach
only the handlers the table routes to that hart at that level; an exception
on top. A level's bound is unknown when its handler's is.

The sum over levels rests on the CLIC: an interrupt preempts only above both
the threshold and the running level `mintstatus.mil`, which the hardware sets
on entry, so lowering the threshold, setting `mstatus.MIE` or restoring
`mstatus` after a critical section lets no interrupt of the handler's own
level in. Only an instruction that lowers `mil` can (`LevelDrop`): a trap
return (`mret`, `sret`, `dret`), or a write of `mnxti` or `mintstatus`. The
trap-entry check refuses one before the handler call; each bound lists those
its root reaches (`Bound::level_drops`, over what is resolved), and
`HartStack::level_drops` gathers a hart's. A hart with none has that half of
the nesting rule checked; one with any is conditional on the rule those
sites may break.

A bound also names what it assumes (`Bound::assumptions`, `Assumption`): the
facts found from the image's types (waker vtables, IPC posts, field types)
rest on the executor invariant, that a task header's `poll_fn` matches its
storage and an erased pointer is read back as its own type; the interrupt
table assumes nothing. `HartStack::assumptions` gathers a hart's. A hart with
a bound and no assumption is proven; with some, conditional on them. The
firmware gate admits exactly the named ones, each until a change proves it.

A bound also lists every function its root reaches over what is resolved
(`Bound::reached`), and each function its floating-point instructions and
floating-point CSR accesses (`FunctionFacts::float_sites`): what checks of a
context's code, such as the firmware gate's interrupt-context checks, read.

The nesting rule's other half, each source at its table level for good, is
the image's configuration. `Dwarf::instances` finds every copy of a function in the
DWARF, out of line or inlined; the firmware gate requires each copy of the
CLIC level and route writers to run inside the functions its contract
allows (start-up, the IPC line's install, the table's routes).

This is the repository's only stack analyzer. The firmware stack gate
(`oer-image-build`'s `stack`) runs it on every
runtime and bootstrap ELF: the interrupt stacks above, and each task stack's
root through `Analysis::bound_with` with the same resolutions. An ELF without
`.stack_sizes` analyses here with observed frames only; the gate refuses it,
and requires a review for every function without a record.

```console
cargo test -p oer-analysis-riscv-stack
```

The optional [stack-call validation](validation/call-spills/README.md) reuses
`FunctionWrites` and `call_writes` to preserve a captured entry-stack word only
through complete disjoint effects. Its companion-ROM diagnostic records why the
selected real publisher copy call still lacks that proof; reviewed frame/call
summaries alone do not establish memory noninterference.
The [alignment-loop validation](validation/alignment-writes/README.md) proves a
finite footprint for one byte-store instruction through the same trampoline and
actual caller arguments. The other eleven unknown store sites still prevent
preserving the selected caller spill across the full call.
The [exit-value validation](validation/alignment-exit/README.md) reuses that
induction witness for an explicit edge postcondition and proves the pointer's
align-up value at the real ROM merge, through both incoming paths. Store effects
and the full-call barrier retain their previous results.
