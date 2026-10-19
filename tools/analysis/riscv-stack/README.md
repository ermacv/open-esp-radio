# RV32 stack bounds

`oer-analysis-riscv-stack` computes worst-case stack bounds of a static RV32 image from
its machine code, over the program model of
[`oer-analysis-riscv-program`](../riscv-program/README.md),
[`oer-analysis-riscv-analysis`](../riscv-analysis/README.md) and
[`oer-analysis-riscv-lift`](../riscv-lift/README.md). It reads the image through one
`ImageFacts` of `oer-analysis-riscv-program` (functions with their aliases,
executable ranges, read-only words, sized data objects, relocated address
words and `.LJTI*` labels, parsed once) and its frame records, relocations
and DWARF through [`oer-analysis-elf`](../elf/README.md), whose RV32 relocation
table decides which relocation is an address word or a control transfer.

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
  that slot of every vtable.
- `Analysis::constant_arguments` reads an argument register's exact value at
  every direct call of a function, complete when `taken_addresses` holds no
  relocation that takes the function's address other than a transfer: how
  the ESP32-S31 image reads every function `Ipc::call_function` posts, the
  targets of the IPC dispatch.

`TypeFacts` reads the image's types, global variables and function
signatures from its DWARF, and `function_pointer_resolutions` sends a call
through a function pointer a static's field holds (the load's address from
the sweep, kept when the load's base register is later reused, or from the
value analysis's registers; the field from the global at that address, merged
and split globals included, down to a pointer to a subroutine type) to its
candidates, matched per parameter and result, never by a signature's
spelling: every function whose address `taken_addresses` lists, with the
field's parameter count, whose parameters and result no known fact
contradicts. For a Rust-ABI pointer a fact is a byte size or a type name;
for a foreign-ABI pointer only a byte size, since C and Rust name one type
differently, and a taken function the DWARF does not describe is a candidate
too. Excluding by name rests on rustc naming one type identically in every
crate and codegen unit, which a test holds the toolchain to. Function merging puts
functions of identical bodies, perhaps of other types, at one address, each
an ELF function symbol with the kept body's size (a symbol of size zero, a
linker script's or an assembly label, names no function of its own): an
address several such functions share matches by the subprogram of any of
them (a merged function keeps one without an address, by its linkage name),
and one without a subprogram makes it a candidate whatever the field. A unit that
states no types at all, such as the precompiled `core` and `alloc` built with
limited debuginfo, names its functions without their parameters or result:
their shape is unknown, and they are candidates whatever the field, except
a trait method (`<Type as Trait>::method`) whose parameter count, which the
trait fixes whatever the type, the image's typed implementations of the same
method state, and which differs from the field's. A unit's `partial + ?`
interrupt stack then names such a function's site as a hole. A site whose
address or field is unknown stays unresolved; a function put into the field
through a transmute is not seen.

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
