# Function and library analysis

Analyze captured code and inspect coverage, symbolic values and explicit gaps.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Function analysis contract

`blobray_application::library::analyze_library(inputs, decoder, memory,
control, visit)` analyzes every function of the given executables in process
and visits one `LibraryOutcome` per function, in input, object and symbol
order:

- `Analyzed`: the function, its records, control-flow coverage and value
  semantics summary; `complete()` requires both to be complete;
- `Blocked`: a function that cannot be analyzed, with the error; its behavior
  is unknown;
- `Gap`: code no function is selected from, such as an object that is not a
  supported RV32 ELF, an unavailable thin member or an executable section with
  no function symbol.

### Function records

`function-records` runs that analysis in its own process and prints the complete
records of the functions it names, so an investigation can read how a vendor
function derives its addresses and values:

```console
blobray --format json function-records --input libpp=/path/to/libpp.a --function lmacAdjustTimestamp
```

Each `--input ROLE=PATH` is an archive or ELF; repeat `--function NAME` for
several functions, and every function of that name in any input is reported.
`--format human` prints each function's record count and completeness, then
its instruction listing: offset, decoded instruction and the symbols its
relocations name (`name+addend`). After `#` each line shows the exact values
the analysis derived at that instruction, in hexadecimal: every register write
other than `zero`, `ra` and `sp` as `reg=value`, and every memory access outside the
entry stack frame as `[address]`, a store with `<- value`. A finite alternative
set is `one-of{a | b}`, a symbol `name+addend`, a section-relative value
`section<index>+offset` and an entry-stack value `sp-offset`. Unknown values and
expressions are omitted, so a missing annotation is no claim either way; the
decoded text itself keeps the disassembler's spelling. An executable image (ET_EXEC) has
no relocations to name its transfers, so for such an input each call or
out-of-function jump to an image address and each taken branch or jump is
annotated `-> name` or `-> name+offset` from the image's sized function
symbols, or with the bare address when no symbol covers it; overlapping
aliases resolve to the one starting last, then the shorter name. These names
are built before the analysis on the operation's one work budget and
deadline, as every Blobray resource contract requires: a reached limit fails
the operation, and a completed analysis is never discarded for them. A
relocation to a `.L` local label in a read-only string section
(`SHF_STRINGS`) shows the escaped text there in quotes; labels of other data,
such as jump tables, get no text. The JSON document is
`{"schema":7,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"functions":[...],"missing":[...]}`:
an `analyzed` function carries its coverage, its value-semantics summary,
`complete` and every record (instructions, blocks, edges, references,
expressions, values, memory accesses, conditions and return values); a
`blocked` one carries the error that stopped it. `missing` lists the names no
input defines, and the command then exits with failure. `--working-memory-mib`,
`--timeout-secs` and `--max-work-units` bound the analysis cooperatively.

### Field accesses

`field-accesses` finds the functions that read or write one field when the
function that owns it is unknown:

```console
blobray field-accesses --input libnet80211=/path/to/libnet80211.a --offset 148 --width 1
```

It analyzes every function and folds the address of each memory access
into a root and a path: a root symbol (named from the function's
references), an entry register, the entry stack, a call result or a section,
then one displacement per loaded pointer and the field's own displacement
last. `sb a4, 148(a5)` after `a5 = *(g_ic + 16)` is `g_ic [16, 148]`;
additions of constants fold into the current displacement. A base the
analysis could not resolve is the `unknown` root with its exact
displacements after it: such an access is a candidate to read, not a proof.
An access whose last displacement is `--offset` (of `--width` bytes, when
given) is reported with its instruction offset, kind and width; absolute
addresses are no field. The JSON document is
`{"schema":5,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"offset":N,"width":W,"functions":[...],"blocked":[...],"partial":N,"partial_causes":{...},"gaps":N,"unknown_addresses":N}`:
`blocked` lists the functions no analysis could read, `partial` counts the
analyzed functions whose coverage or value semantics is incomplete and
`partial_causes` breaks them down by cause (see
[interfaces and formats](../interfaces-formats/README.md)), `gaps`
counts the code no function covers and `unknown_addresses` the accesses whose
address is not known at all. Any of them means the list may be incomplete.

### Call arguments

`call-arguments` reports the argument registers `a0`..`a7` at every call site
of the named functions, so writes made through calls such as
`phy_i2c_writeReg(block, host, register, value)` become a query:

```console
blobray call-arguments --abi riscv-integer --input rom=/path/to/rom.elf --symbol phy_i2c_writeReg_Mask
```

In a relocatable object a call site is a `call` or `branch` relocation to the
name, and its register state is the analysis' `CallInputs` record at the
`jalr` four bytes after a `call` relocation's `auipc`, or at a `branch` (JAL
or conditional branch) relocation's own offset; a site never takes another
transfer's state, so a jump the analysis kept no state for has no arguments. An executable image has no relocations:
a transfer to the start address of a sized function symbol of that name is a
call site. Each argument is a constant, an image address, a symbol plus
addend, a section- or entry-stack-relative value, a finite alternative set,
the caller's own argument at entry (`arg0`..`arg7`), another entry register,
or unknown (`?`); a site without retained register state has no arguments.
These are may-values of one site: there is no interprocedural expansion and
a value does not prove that its path runs. The JSON document is
`{"schema":4,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"symbols":[...],"callers":[...],"blocked":[...],"partial":N,"partial_causes":{...},"gaps":N}`;
`partial` counts analyzed functions whose coverage or semantics is
incomplete, which can hold further sites or less precise values, and
`partial_causes` breaks them down by cause.

### Calling convention

Without an assumption, every register is unknown after a call the analysis
cannot follow (every call, since callees are not expanded): a field read
through `s0` after a call has no address and is missing from
`field-accesses`. `--abi riscv-integer` on `function-records`,
`field-accesses`, `callers` and `register-accesses` assumes the RISC-V integer
calling convention instead: such a call keeps `sp`, `gp`, `tp` and `s0`-`s11`,
and `a0`/`a1` become the call's result. It is an explicit assumption, never
inferred from ELF flags, and every JSON document records it as `abi`
(`"riscv-integer"` or `null`). On the pinned ESP32-S31 `libpp.a` it turns
8000-odd accesses at unknown addresses into known ones.

### Returned constants

`--call-returns` on every analysis command (`function-records`,
`field-accesses`, `callers`, `call-arguments`, `register-accesses`,
`mmio-diff`) takes the constants a called function returns at every call to
it. A first pass analyzes every function and keeps, for `a0` and `a1`, the
constants each one returns when its analysis is complete, every exit is a
return and every return leaves at most eight known constants there; a tail
call or another jump out keeps none. The second pass gives a call those
constants instead of an unknown result, one call level deep. A call names
its callee by address in an image (a direct `jal`, or a register holding a
known address) and by a global symbol in a relocatable object; a symbol
defined with different results answers nothing. The first pass makes the
analysis take about twice as long: on the former HIL correctness image it resolves no
further address, while in the ESP32-S31 inventory it gives the ROM's
`phy_pbus_rd` its PHY_PBUS read windows through `phy_pbus_rd_addr`. It is an
explicit assumption, and every JSON document records it as `call_returns`.

### Read-only windows

A linked image is analyzed by its sections, as a firmware packer loads it
([static RV32 images](../../../../riscv/program/README.md)), so an ESP-IDF
firmware ELF, with its IRAM code and DRAM data in one segment, its flash
rodata placed after a reservation that overlaps code and its thread-local
template, is analyzed whole. ESP-IDF flags its flash rodata writable, so no
constant would be read from it: `--read-only START:LENGTH` on every analysis
command asserts a range read-only at run time whatever the section flags say,
and constants and jump tables are read from the sections it covers whole. Take it
from the chip's memory map, the flash window (on ESP32-S31 the XIP window
`0x40000000:0x4000000`); Blobray never infers it from section names. It is an
explicit assumption, and every JSON document records it as `read_only`.

```console
blobray register-accesses --abi riscv-integer --read-only 0x40000000:0x4000000 --range 0x20000000:0x1000000 --input image=phy_tracking_reference.elf
```

### Callers

`callers` lists every reference the analyzed functions make to the named
symbols:

```console
blobray callers --input libpp=/path/to/libpp.a --symbol pm_scale_listen_interval
```

A reference is a relocation: in a relocatable object every call, jump and
address of another symbol carries one, so a `call`, a `branch` and an
`address` (a function pointer stored in a table) are all found with the
referencing instruction's offset. Repeat `--symbol NAME` for several targets.
The JSON document is
`{"schema":3,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"symbols":[...],"callers":[...],"blocked":[...],"gaps":N}`.
A call inside one section that the assembler resolved without a relocation
is not a reference and is not listed.

Symbol names in every JSON document are strings when their bytes are UTF-8
and byte arrays otherwise.

Selection takes static and dynamic `STT_FUNC` symbols defined in nonempty
executable sections of RV32 ET_REL objects or static ET_EXEC images. Aliases
and occurrences in different tables remain separate functions even when their
names, addresses and bytes match. ET_REL addresses are section-relative;
ET_EXEC addresses belong to the image. A nonzero symbol size supplies the
extent; a zero-sized symbol is blocked with `needs-extent`, and no
neighboring-symbol heuristic is used. No function boundaries are inferred from
disassembly.

Analysis uses a bounded iterative worklist rooted at the entry. Calls do not
expand into callees; continuations are possible control flow, not proof that a
call returns. Unknown instructions, indirect transfers and uninterpreted
relocations preserve separate decoding/control-flow/reference gaps. Unvisited
bytes are reported without claiming they are instructions or dead code. There
is no execution or equivalence claim, no interprocedural engine and no
computation reused between calls.

Domain owns values and the ISA port; artifacts lends captured code and
structural ELF facts; `blobray-backend-riscv` interprets instructions and
relocations; `blobray-analysis` builds local graphs and coverage. Application
enumerates objects, prepares each object once for all its functions and owns
the visit. No analysis or backend module opens a path. Graph allocations
require working-capacity admission.

The artifact view lays an image's memory out by its loaded sections
([static RV32 images](../../../../riscv/program/README.md)): they must not
overlap, each must be the bytes a load segment carries at its address, and
selected code must agree with an executable section in an executable segment.
A thread-local template and a segment holding both code and data sections
are accepted; dynamic loading, runtime relocations, RV32E/quad-float ABI and
writable code sections are outside this static profile. Static relocation
sections retained by `--emit-relocs` are provenance; their edits are not
applied again during value/flow interpretation. Constants come from sections
neither writable nor executable, and from writable sections only inside the
caller's [read-only windows](#read-only-windows), an assertion about the
chip's memory map that no section flag makes. Single-precision
instructions are decoded and lifted for their integer and memory effects only;
the FP register file is not modeled. Concrete execution uses the separate
[execution contract](../execution/README.md#concrete-execution-and-comparison).

### Structural decoding

Decoder policy 1 uses pinned `rv-asm 0.2.1` through an independently injected RV32 decoder.
It supports its RV32IMAC encodings, the backend's Zba/Zbb/Zbs, Zcb, Zcmp and
single-precision F forms, and records unsupported encodings as gaps.
Printed instruction text is versioned with that decoder, not a parsing interface;
bytes, offsets and typed flow are the structural interface. Calls use ABI link
registers x1/x5; the decoder marks returns (a `jalr` through `ra` or `t0` at
offset zero that links neither, and the returning Zcmp pops) as `return`
flow, which describes the calling convention, not proof about the dynamic
register value. Other indirect transfers remain unresolved. A direct call records its target but does not enqueue the callee.

Records include instructions, maximal decoded basic blocks, instruction-origin
edges, relocation references and gap ranges. Block IDs are their start offsets;
edge `from` is the controlling instruction offset, not a guessed load address.
Direct fallthrough,
taken branches, jumps, calls, possible continuations, returns, stops and conflicts
are distinct. Unvisited bytes can coexist with complete reachable decoding;
gap records also explain explicit unsupported or conflicting boundaries.
ELF `$d`/`$x` mapping symbols prevent decoding declared data as instructions.

References retain relocation section/index/type/addend and original symbol
identity, raw name bytes, binding and definition kind. Interpreted references keep
separate target and HI/LO pairing evidence. CALL/CALL_PLT, branch/JAL/RVC branches,
absolute HI/LO, PC-relative HI/LO and R_RISCV_32 are recognized. HI/LO pairing uses
relocation identity, never adjacency or equal names. NONE/RELAX/ALIGN remain
metadata, including validated null-symbol references. REL implicit addends and
other relocation types remain uninterpreted rather than being filled with zero.
Named external references need no chosen implementation or final address.

Coverage has independent `decoding`, `control_flow` and `references` fields for
this local scope. A partial function is an analyzed outcome, not a failure.
Missing extents (`needs-extent`) and unsupported selections block that function
only; integrity failures and exhausted limits fail the whole call.

Memory admission includes one full ELF object, a 1 MiB operation envelope,
the concrete relocation/symbol structure sizes and retained name capacities,
32 bytes per ELF mapping symbol for mapping indexes, and 512 bytes per possible instruction
halfword for graph state. Value analysis additionally admits the actual type sizes
of 32 register values, operation metadata, a queue index and membership flag per
decoded instruction. These conservative reservations bound retained buffers;
host allocator overhead and decoder/library internals remain under process
containment. Names are capped at 4096 bytes and serialized records/manifests at
64 KiB. Graph queues are marked before insertion and cannot grow through cycles.
A function's records are collected into admitted memory, presented to the
caller's visit and released before the next function.

### Values and memory effects

Function records carry register values, memory accesses, transfers and
semantic gaps beside the structural records.

The domain `FunctionSemantics` port extends decoding with typed operations and
relocation roles. The RISC-V backend lifts decoded instructions, never display
text. Analysis owns an iterative fixed-point computation over the existing CFG;
application owns admission and the caller's visit. The computation borrows
captured bytes, an ISA port, working capacity, control and a sink. It cannot
obtain filesystem or hardware-description access.

Register values are unknown, RV32 constants, image virtual addresses, section-relative addresses, exact
symbol references with addends, offsets from the entry stack pointer, or flat expression IDs. Entry
`x0` is zero, `sp` denotes its entry value, and other registers have symbolic entry values.
Stack-relative values express provenance, not allocated or accessible memory.
Exact incoming values join into canonical sets of at most eight alternatives.
The leaves are constants, image/section addresses, physical symbols and entry-stack
offsets; sets cannot contain other sets, expressions or unknown leaves. Arithmetic
uses bounded Cartesian products and immutable loads read every candidate. These
are may-values: branch correlation is not retained and membership does not prove
that a runtime path selects that value. A ninth distinct result widens to unknown
and emits `alternative-limit`, making semantic completeness false. Unequal
symbolic expressions and incomplete relocation uppers still join to unknown.

Alternative storage and its lookup index belong to the analysis phase and consume
admitted memory and work; exhausting either fails the call. JSON uses
`{"kind":"alternatives","values":[{"kind":"image-address","address":4096},{"kind":"image-address","address":8192}]}`;
human output uses `one-of{... | ...}`. Analysis never selects one of these
callees or composes it as a definite call. Expressions retain alternative operands.
Branches are not pruned and computed indirect destinations do not extend the CFG,
with one exception the build's facts prove: a compiler jump table.

### Jump tables

A compiler dispatches an RV32 `switch` through a read-only table:
`lui`/`addi` of the table, `slli index, 2`, `add` (or `sh2add`), `lw`,
`jalr zero, 0(target)`. The value analysis records each such jump as a
`dispatch` (`{offset, base, index, scale}`): the target is the word loaded
from `base + (index << scale)`, the load, addition and scaling in the jump's
own block; `base` is the constant, image address or object symbol the
addition reads, so a base the block or a loop's preheader forms gives the
same record on every pass even when the index merges to an unknown value.
A dispatch's table is proven by the build's facts (`oer-riscv-model`'s
`TableEntries` port):

- In a relocatable object (`oer-riscv-program`'s `ObjectTables`), a table has
  no label, so the bounds check guarding the dispatch states the entry count
  (at most 4096): the dispatch block's only incoming edge must be the
  in-range side of a conditional branch on the same, known index against a
  constant, the fall-through of `bltu limit, index` / `bgeu index, limit` or
  the taken edge of `bltu index, limit` / `bgeu limit, index`; a comparison
  elsewhere in the function bounds nothing. Every selected entry must carry
  an `R_RISCV_32` relocation with an explicit addend (`SHT_RELA`; a `SHT_REL`
  table keeps its addends in the words and is not proven) to a label inside
  the function.
- In a static image linked with `--emit-relocs` (`ImageTables` over the
  image facts), a table the compiler labelled (`.LJTI*`) is the run of words
  the link kept relocated from its label to the next symbol of its section,
  with or without a bound: the dispatch of a Rust `async` state machine,
  whose state index no check bounds, is followed. A sized read-only data
  object is its words; with a bound, the bound's count of read-only words.
  These need no relocations: an image linked without them proves every
  such table but a labelled one no check bounds.

Every entry must land inside the function. The function is then analyzed
again with those targets as jumps; passes repeat while each proves again
every table it followed and more, so a dispatch reached only through another
table's case (a nested `switch`) is followed too. A pass that no longer
proves a table it followed proves none: the function is then reported
without following any table. The recognizer and these passes are
`oer-riscv-analysis`'s (`RecognizedTables`, `research_following_tables`); the
stack analyzer runs the same driver. The facts this needs, the recognizer's
indexes and the found entries are held in admitted working memory, and
every record the recognizer visits is charged to the work budget; a return
(the decoder's `return` flow) is never a dispatch. A dispatch whose table the facts
do not prove stays an indirect gap. `function-records` lists each followed table
as `jump_tables` (`{site, first_case, entries}`, entry `i` being case
`first_case + i`, where `first_case` undoes GCC's subtraction of the smallest
case from the switch value, seen through an addition of a constant on either
side or a subtraction under any 8- or 16-bit zero extension; `first_case` is
`null` for any other index shape, and then no case value is claimed), and the
human format prints `jump table +site: +target <- cases …` with those case
values, or `<- entries …` with entry indexes when they are unknown, after the
listing.

Loads retain address and width. In the static ELF image profile, the bytes of
loaded sections neither writable nor executable supply constants, with signed
byte/halfword extension, and so do writable sections inside the caller's
[read-only windows](#read-only-windows). Other writable memory, code, zero-fill
(`NOBITS`) and unmapped addresses remain symbolic loads; atomic load results remain unknown. Stores retain address, width and source value. Atomic operations retain their read/write kind and
conditional store behavior without modeling memory or reservations. Calls are
opaque effects: their possible continuation forgets registers except x0 and symbolic x10/x11 call results. Unknown operations and conflicting instruction
boundaries are explicit gaps and cannot propagate stale values. Missing decoded
regions remain structural gaps. Semantic coverage is separate from value
precision: an unknown input does not make a modeled instruction unsupported.

HI/LO address formation requires compatible relocation identities and an actual
flowing upper value. PC-relative pairs use their recorded label relationship;
a partial pair never becomes a complete address. RV32 integers wrap according to
the ISA; symbolic address arithmetic keeps provenance only for representable
transformations. MMIO naming, call composition, mutable memory forwarding, table models and machine execution remain outside this profile.

The solver reserves input-dependent state and queue capacity before allocation,
keeps at most one queued item per node, and charges every transfer to the shared
work/deadline budget. Fixed-point records are emitted once in offset order.
Resource exhaustion fails the call with the phase it reached. Working-capacity
admission is not a claim that the process performs no system allocations.

During `analyze-values`, the run position's `table` is the code section index and
`entry` is the current instruction's section offset or image virtual address. The integer
semantics follow the
[RISC-V integer ISA](https://docs.riscv.org/reference/isa/unpriv/unpriv-index.html)
and [RISC-V ELF relocation ABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/).
Known-address counts include symbolic and stack-relative expressions; they do
not imply a final load address, valid mapping, alignment or successful access.

Address expressions denote RV32 base-plus-offset arithmetic modulo 2^32; their
signed metadata offsets use checked i64 arithmetic. An overflowing metadata
expression becomes unknown. Both linked calls and relocation-identified tail
calls are opaque effects at the transfer instruction; their AUIPC upper alone
is not a resolved address or an unknown relocation diagnostic.

## Explicit ROM companions

Synthetic linking does not treat a linked ROM ET_EXEC as a relocatable input.
Instead a `LinkRequest` selects exact ROM functions or data objects as
external definitions in `companions`, by `SymbolId` in a companion executable
given beside the link inputs.

`LinkRequest.absent` names at most 64 undefined symbols no given input
defines, such as a C library routine only a never-reached diagnostic path
calls. Each resolves to the unmapped `ABSENT_SYMBOL_ADDRESS`, so executing or
accessing it stops with an execution gap; an absent name that is also a
companion fails.

At most 64 companion definitions are allowed; undefined symbols, symbols that
are neither functions nor data objects, duplicate names, collisions with link
input definitions and addresses inside the synthetic placements fail.
Definitions are validated against captured executable bytes; no stub
implementation is emitted. This address-only acquisition checks the exact
physical symbol table, declared nonzero function extent, allocated code section
and unique executable file-backed load mapping. Non-executable data sharing the
virtual address, TLS/dynamic metadata and executable mappings elsewhere in the
carrier do not become a runtime dependency. A definition neither loads that
carrier nor proves its code can execute under the static-image profile.
A data object (`STT_OBJECT`) needs a nonzero extent inside one non-executable,
non-TLS `PROGBITS`/`NOBITS` section with a nonzero address. ROM interface
storage is often declared without `SHF_ALLOC` or a load mapping, so neither is
required: the definition grants the address only, never the data bytes.
Code placement uses ELF allocation/execution flags, including vendor sections
whose names are not `.text`. Other unresolved symbols remain linker failures.

`linking::propose_companions` finds the definitions a request needs instead of
a hand-kept list. It trial-links the request, with its explicit companions
applied and unresolved names permitted, reads every name the closure leaves
undefined from the trial image, and resolves each one in the candidate
executables in the given order. A name resolves to the first candidate that
defines it. Within that candidate a single global or weak function or data
object is taken, or a single local one when no global or weak one exists.
Several definitions of the chosen binding, or none in any candidate, leave the
name unresolved. Candidates must be distinct executables outside the link
inputs. The proposal grants nothing: the caller copies the exact selections
into its link request. A hidden undefined name cannot be left for a companion
and fails the trial link.

Definitions do not map ROM into writable memory or authorize execution.
Concrete comparison uses the separate
[execution contract](../execution/README.md#concrete-execution-and-comparison).

## Final-image target audit

```console
cargo blobray audit-targets --artifact firmware.elf --forbid radio=0x2f800bf0..0x2f8016bc --format json
```

`audit-targets` reads one RV32 static ELF in process and identifies the
inspected bytes by SHA-256. The artifact owner visits
every executable section, including code without function symbols. A section-less
ELF cannot produce a vacuous pass. The shared artifact mapping parser validates
local, zero-sized `$d`/`$x` symbols (including `$xrv32…` ISA-qualified code),
rejects conflicting/out-of-section mappings and non-RV32 code markers,
and separates embedded data from instructions. Ordinary labels never authorize
skipping bytes. Every section must match its executable file-backed load range.
The common RV32 decoder/lifter and integer
constant evaluator provide the analysis.

The linear scan records direct branch/jump targets and locally resolved JALR
targets, resetting values at data intervals, control transfers and unknown instructions. Unresolved
indirect transfers are counted separately: a clean result only answers the declared
statically resolved target policy, not absence of all dynamic calls. Unknown major
opcodes that might conceal a transfer, unsupported instruction lengths and missing
coverage make the result unclean. Recognized non-control opcode classes can be
skipped with a counter and a full value reset. CSR accesses are non-control; trap
returns count as unresolved indirect transfers. This classification does not add
CSR or privileged execution semantics. Truncation is an integrity error.

Each finding retains section, site, target and selected forbidden range. The JSON
document (schema 1) records the audited digest, decoder and semantics identity,
policy ranges, records, summary counts and the `pass`, `fail` or `inconclusive`
verdict.
`executable_bytes` counts the whole executable section; `embedded_data_bytes`
separately counts bytes excluded by validated mapping symbols. Coverage-gap
records retain the site, reason and at most four encoding bytes.
A policy violation or coverage gap exits nonzero with the document on stdout;
resource and integrity failures print no document. `--working-memory-mib`,
`--timeout-secs` and `--max-work-units` bound the audit.
Ranges are half-open and the request accepts 1–64 named ranges.
