# RV32 stack bounds

The opt-in stack-address context
connects canonical machine setup and call inputs to numerical word-proof
premises. It is scoped to an image capture and invocation; placement or a
partial stack budget alone never establishes a current-SP interval.

`oer-image-stack` computes worst-case stack bounds of a static RV32 image from
its machine code, over the program model of
[`oer-analysis-riscv-program`](../../analysis/riscv-program/README.md),
[`oer-analysis-riscv-analysis`](../../analysis/riscv-analysis/README.md) and
[`oer-analysis-riscv-lift`](../../analysis/riscv-lift/README.md). It reads the image through one
`ImageFacts` of `oer-analysis-riscv-program` (functions with their aliases,
executable ranges, read-only words, sized data objects, relocation-visible
address uses, checked address words and `.LJTI*` labels, parsed once) and its frame records, relocations
and DWARF through [`oer-analysis-elf`](../../analysis/elf/README.md), whose RV32 relocation
table decides which relocation is an address word or a control transfer.

## Code-generation preconditions

The Waker rule in `src/wakers.rs` selects a vtable slot from the innermost
DWARF scope at the unresolved call, or at every reaching target load when
the call's scope has ended. That scope must retain the identity of the
operation that produced the instruction. The image pipeline fixes these
conditions in its [compiler setup](../build/README.md#code-generation-preconditions):

| Condition | Why the scope proof needs it |
| --- | --- |
| `-Zmerge-functions=disabled` | LLVM MergeFunctions can merge identical bodies from different source functions and discard one DWARF origin. |
| `-Cllvm-args=-enable-machine-outliner=never` | Machine outlining creates functions without the original instruction scopes; the disable applies regardless of RISC-V features or optimization level. |
| `pick-merged-source-locations` absent | Picking one merged origin can assign both wake and drop paths to `Waker::wake`, incorrectly excluding the drop slot. |
| LLD `--icf` absent (default off) | Identical code folding can put different symbols and source origins at the same machine bytes. |

The pipeline validates its complete rustc flags, including Cargo's encoded
form, and the linker validates its expanded arguments. Violations are typed
build errors. This is a precondition of the proof, not a property the analyzer
can recover from arbitrary ELF bytes: other producers must establish it too.
LLVM's debug-location correctness, complete vtable/relocation evidence and
the unsafe-memory invariants below remain trusted boundaries. A lost or
broadened scope does not itself establish a Waker method; the existing
scope/load checks still have to identify the slot. Prebuilt compiler-runtime
code retains its separate [coverage boundary](../../../docs/unsafe.md).

## Analysis facts

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
`ResourceLimited` instead of running on. A local expression-pool limit instead
marks the function partial and retains `FunctionFacts::expression_limited`.
`Analysis::expression_limited_functions` counts these functions; image stack
reports print the count when nonzero. The queries over an image's facts
(`stack_sizes`, `taken_addresses`, `waker_vtable`, `interrupt_table`,
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
and `noreturn` follows subprogram declaration/origin links only when the
concrete entry omits the attribute, requiring agreement at each address.
Present false or malformed flags, broken links and cycles do not prove noreturn.
Sharing expression decoding and source-reference selection does not unify
those interpretation policies. Retained facts and temporary
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
local control-flow graph is complete. Calls retain their continuation under
the psABI even when their target is unresolved, provided that continuation
stays inside the function; this proves only the caller's frame. Its bound
remains unknown until every callee is resolved and bounded. Fall-through
outside the function, conflicting instruction boundaries, stop edges, decode
gaps, unknown outgoing jumps and dynamic `sp` prevent an observed frame.
A sole open indirect exit whose value analysis
proves every target external is fed back through `KnownJump` for one complete
graph analysis; neither its frame nor its callees receive an assumed bound.
Its transfers are every direct call, out-of-function jump and external taken
conditional branch of a linear sweep over its whole extent, which also
reaches code behind jump tables the graph does not expand, plus the indirect
transfers whose target the value analysis resolves; an indirect jump whose
targets the analysis finds inside the function is a jump table, not a
transfer. A call or jump through `table[index]`, whose base is a constant or a
register the value analysis knows at the transfer, reaches each entry of the
table when its length is exact: the index's bounds check on the path to the
dispatch (`bltu limit, index` or `bgeu index, limit` against a constant) or
mask (`andi`), or the size of a data object the base starts in a section
neither writable nor executable, against which the language bounds-checks the
index (a table in code loaded into RAM may be rewritten at run time, so its
file bytes are no constant). Labelled relocation tables also cross-check every
entry against the image's read-only words; a label alone cannot admit mutable
storage. A captured indexed address can survive calls in a saved register:
its candidate dispatch uses the same table extent and target proofs on every
table-following pass. A zero entry is an empty
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
An external conditional branch is a tail transfer, like a direct jump out
of the function: the destination's bound counts below the source's retained
stack depth, while the fall-through path remains in the local graph.

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
different paths fail. Before an entry sets `mstatus.MIE` (admits nesting) its
frame must hold `mepc`, `mcause` and `mstatus` as the trap left them; it
changes `tcontrol` only once the frame holds it, and it calls its handler
with the FPU off (`mstatus.FS` cleared). No trap saves the floating-point
state, so any floating-point instruction fails, and so does a write of a CSR
other than `mscratch`, `mstatus`, `mepc`, `mcause`, `tcontrol` and the CLIC
threshold. `TrapEntry::frame` is the deepest `sp` below the interrupt stack's
position on any path up to the handler call: what every nesting level adds
beside its handler's bound.

`interrupt_entry` checks an interrupt's entry the same way and then its exit,
from the handler's return to `mret` on every path. The handler leaves the
frame, `mscratch` and the callee-saved registers as it found them; every
other register and `mepc`, `mcause`, `mstatus`, `tcontrol` and the CLIC
threshold are unknown. The exit must clear `mstatus.MIE` before it restores
`mepc`, `mcause` (whose `mpil` is the level `mret` returns to), `tcontrol`
(whose MPTE is the trigger enable `mret` returns to) or the threshold, never
set it again, keep `sp` within `TrapEntry::frame`, and reach `mret` with
every register, `sp` and `mscratch` at their values at the trap and those
CSRs restored from the frame; a value the frame holds is followed through
its loads, and a flag it holds decides a branch. A lost or stale
`mepc`/`mcause`/`tcontrol`, a wrong `sp`, an early or repeated `MIE`, a
deeper exit, the FPU left on or touched, another CSR written or a register
not restored fails the check (`trap_exit_tests.rs` checks the staged
runtime's own `vectors.s`). The compiler fixture reuses the runtime's
`global_asm!` invocation, `TrapFrame` declaration, architectural definitions
and generated Rust vector tables and markers. Its operands therefore come
from the same `size_of`, `offset_of` and constants as the runtime. Mutations
name the assembly's operands and must match before compilation; the entry
and exit checks reject the resulting machine code.

Facts outside the machine code close indirect sites through
`Analysis::bound_with` and its `Resolutions`, which apply only where the
analysis left a target unresolved. Each site's targets carry the `Fact`s that
gave them; the facts of one site unite, each being a superset of what the
site reaches. An empty set resolves a site only where its fact is complete
for it (`Fact::proves_empty`: the interrupt table, no tasks); an empty
set of found facts (the waker vtable, field types) is a hole,
`Reason::NoCandidate`, never a site that reaches nothing:

- `interrupt_table` reads an image's interrupt table (the slice an exported
  symbol holds, its pointer word and every non-zero handler word carrying the
  relocation that names it) in the chip's `TableLayout`.
- `Dwarf` reads the chain of functions inlined at an address and what each
  function returns (`Returned`): its own return type or, through
  `DW_AT_abstract_origin` and `DW_AT_specification`, its origin's; nothing
  in a unit that describes types; unknown in a unit that describes none (a
  library built with line tables only, such as the precompiled `core`),
  without DWARF, or where descriptions disagree. `waker_vtable` takes a
  census of the image's `RawWakerVTable`s. Every `Waker` carries a
  `&'static RawWakerVTable`: `Waker::from_raw` and `Waker::new` (both
  unsafe) take one, `Waker::noop` carries `core`'s own, and `Clone` copies
  the one a clone function returns. The census finds vtables by type and by
  layout. By type: a function the DWARF says returns
  `core::task::wake::RawWaker` is a clone function, and every use of its
  address the link kept (`--emit-relocs`), in code and in data of every
  section, writable (`.data`, SRAM or PSRAM) or not, must be the first of
  four relocated function pointers in data whose other three return no
  type. By layout: four relocated function pointers in a row in data whose
  first function's return is unknown are a vtable when that function's code
  takes the row's address, as a clone function of a static vtable does;
  otherwise the row fails the census. Every static the DWARF types as or
  containing a `RawWakerVTable` (`.bss` included) must hold a vtable found
  so, unless the link discarded it (its location lies in no allocated
  section). A clone function's address taken anywhere else (code that could
  build a vtable at run time, a lone pointer, a relocation naming no
  address), a row the census cannot classify, a typed vtable the relocations
  do not show, or a second vtable fails the analysis with `Integrity`; so
  the image holds at most one vtable, the executor's, and every waker it
  builds carries it. `Fact::WakerVtables` therefore names no `Assumption`;
  the census itself rests on what it reads being whole:
  - the DWARF: a function it gives no return type is found only where its
    address heads four function pointers in data, so a vtable built at run
    time from such a function (its address taken by code), or one inside an
    executable section, is not found;
  - the relocations: `--emit-relocs` keeps every one the link resolved, each
    `HI20`/`LO12` pair the linker's relaxation rewrote included;
  - memory safety: unsafe code, the image's and its dependencies', does not
    forge a `RawWakerVTable` or write the vtable pointer inside a waker by
    other means than the constructors above.

  `waker_resolutions` sends an
  unresolved call inside `Waker::wake`, `wake_by_ref`, `drop` or `clone` to
  that slot of the vtable; without one the site keeps an empty set, a hole.
  When the inline scope ends before the call,
  every reaching target load in a closed local control-flow graph must
  identify a method and its matching slot. A merge of methods keeps the
  union of their slots; an unknown producer leaves the call unresolved.
  An unresolved indirect tail can also use these loads when its own exit
  is the graph's only open site. The resolution is accepted only when every
  selected slot of the vtable targets an address outside the containing
  function; otherwise the tail could add missing local predecessors.
- `Analysis::constant_arguments` reads an argument register's exact value at
  every direct call of a function, complete when `taken_addresses` holds no
  relocation that takes the function's address other than a transfer.

### Waker calls from typed locations

`WakerVtables` also trusts the correctness of the compiler's DWARF type
identities, member layouts and variable/parameter location lists, just as
the method-scope rule trusts its inline scopes. This is a premise of the
proof, not a guarantee for arbitrary ELF or incorrect debug information;
it introduces no additional `Assumption`. Missing or unsupported metadata
leaves the site unresolved.

With `read_debug(..., DebugMetadata::TypedLocations, ...)`,
`waker_resolutions` can prove a target load without a Waker method scope.
Its `TypeFacts`, `Dwarf` and `Analysis` must describe the same ELF. It
projects the complete four-byte `vtable` field of exactly
`core::task::wake::Waker` or `core::task::wake::RawWaker` from the DWARF
layout. A register piece must cover the entire byte interval from the
chosen base witness through the end of `jalr`, including every intervening
PC. Adjacent entries for the same variable and piece may join; a gap,
another variable, an incomplete piece, a spill or an unsupported expression
cannot extend that interval. User types with the same unqualified names
supply no proof.

`core::option::Option<core::task::wake::Waker>` is accepted only when DWARF
describes a two-variant null niche at that same vtable word and a local
machine branch proves the word nonzero on every incoming path. An
unselected variant supplies no initialized Waker field.

The retained local graph and shared instruction semantics establish that
every reaching target load uses that vtable value plus a constant, through
register copies, `li`, `addi` and `add` only. RV32 additions wrap; every
resulting displacement must be exactly 0, 4, 8 or 12, and the DWARF
`RawWakerVTable` members must confirm those four slots. The existing census
supplies the sole vtable and the reaching-target proof connects each load
to the transfer. Joins retain all possible slots. An unknown input,
unsupported arithmetic, memory write, call boundary, cycle or the bounded
128-step backward search leaves a hole. The existing value-identity
`RegisterWitness` contract is unchanged.

A closed graph is required, with the existing sole-open-tail exception
only when all selected targets lie outside the function. Multiple open
tails remain unresolved. No source name, source line, nearby scope or MIR
call list substitutes for these machine and location facts.

`task_poll_resolutions` finds the executor's task polls: the executor
(`oer-espressif-executor`) polls each declared task by a direct call of its
never-inlined poll boundary, whose address each task exports in a `#[used]`
record (`TASK_POLL_RECORD`, `__OER_TASK_POLL.<module path>`, one relocated
read-only word). Every direct call of a recorded boundary is a task poll site
(`Resolutions::task_polls`) and the boundary is the task's root
(`Resolutions::task_roots`); no function name is read. A record whose
boundary no direct call reaches (an inlined poll, or a task never polled),
or an image with the executor's waker vtable and no record, fails the
analysis. The calls are direct: a task poll assumes nothing, and an
executor's bound reaches only its own tasks.

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

Both use `audit_function_addresses` and exactly one candidate rule: a
function leaves a call site's candidates only when every place that takes its
address provably keeps its type. Each
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
leaves candidate sets too broad for useful bounds. It does not model
interprocedural pointer propagation or constructor summaries.

`interrupt_stacks` bounds each hart's interrupt stack (`Stacks`): one
interrupt per level the hart takes (its table entries' levels), each the worst hardware-vector entry's frame plus its
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
facts found from the image's types but not enumerated whole (field types)
rest on the executor invariant, that an erased pointer is read back as its
own type; the interrupt table and the executor's direct task polls name no
assumption, nor does the waker vtable census, which rests only on the
completeness of the DWARF and relocations and on memory safety (above).
`HartStack::assumptions` gathers a hart's. A hart with
a bound and no assumption is proven; with some, conditional on them. The
firmware gate admits exactly the named ones, each until a change proves it.

A bound also records every indirect site its walk took (`Bound::sites`, a
`TakenSite` by site): how many targets the site reaches and where they come
from (`SiteSource`). The machine code's own resolutions are a `CodeFact`
(`JumpTable`, a table of exactly known entries; `ValueAnalysis`, a target
register the value analysis knows), which the analysis keeps per function
(`FunctionFacts::code_facts`) and which assume nothing; the others are the
site's `Fact`s, each with its `Fact::assumption`
(`SiteSource::assumptions`). `Bound::assumptions` is the union of the
assumptions of these sites.

A bound also lists every function its root reaches over what is resolved
(`Bound::reached`), and each function its floating-point instructions and
floating-point CSR accesses (`FunctionFacts::float_sites`): what checks of a
context's code, such as the firmware gate's interrupt-context checks, read.

`Bound::predecessors` retains one caller/callsite edge at first discovery of
each non-root function. `Bound::witness` reconstructs a root-to-function path
through these parents without searching call edges again. It returns no path
for a function beyond an unresolved transfer. This first-discovery path need
not be shortest and is independent of the deepest stack path in `Bound::path`.
Bound builders take the caller's `Ledger` and `RunControl`; predecessor entries
are admitted before allocation. Cloned interrupt bounds share their immutable
predecessor map.

`Analysis::reached_cut(root, resolutions, cut, keep, control)` uses the same walk as
`Bound::reached`, stopping at transfers whose instruction sites are in `cut`.
The function owning a cut site remains reached. `task_poll_sites` obtains the
task poll sites from `Resolutions`; their direct targets are task roots. The
image gate uses each hart/level's own `Stacks::resolutions_for`, including
its interrupt routes, for both the bound and the cut. A function reachable
without crossing a task poll remains strict even if a task also calls it.

The nesting rule's other half, each source at its table level for good, is
the image's configuration. `Dwarf::instances` finds every copy of a function in the
DWARF, out of line or inlined; the firmware gate requires each copy of the
CLIC level and route writers to run inside the functions its contract
allows (start-up, the table's routes).

This is the repository's only stack analyzer. The image build pipeline
(`oer-image-build`) invokes the gates in `oer-image-checks`:
[`oer_image_checks::stack`](../checks/src/stack.rs) checks the runtime
task stacks and the bootstrap stack, and
[`oer_image_checks::interrupts`](../checks/src/interrupts.rs) checks
the runtime's interrupt stacks above. The runtime gates share one analysis;
each task stack's root uses `Analysis::bound_with` with the same resolutions.
An ELF without `.stack_sizes` analyses here with observed frames only; the
gate refuses it, and requires a review for every function without a record.

```console
cargo test -p oer-image-stack
```

The optional stack-call research reuses
`FunctionWrites` and `call_writes` to preserve a captured entry-stack word only
through complete disjoint effects. Reviewed frame/call
summaries alone do not establish memory noninterference.
