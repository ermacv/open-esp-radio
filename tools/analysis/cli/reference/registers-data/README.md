# Registers and captured data

Inspect MMIO candidates and captured data.

[Choose a task](../../../blobray-README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Captured data, tables and coefficients

`oer_analysis_blobray_application::data::export(request, executables, memory, control)`
reads exact ranges from one captured object in process. It does not run an
analyzer or infer a table boundary.

A `DataRequest` has the `object` (`ObjectId`: the containing executable's
content and the standalone or archive-member location), an optional anchor
`symbol` and 1–32 `ranges`. Copy the object identity from an inventory; it
retains the archive member ordinal even when names and bytes repeat. The
executable the object names must be among `executables`. Each range uses one
of these selectors:

| Selector | JSON fields in addition to `kind` | Meaning |
| --- | --- | --- |
| `section` | `section`, `offset`, `length` | Explicit section-relative bytes |
| `symbol` | `symbol`, `length` (integer or null) | Exact static or dynamic SymbolId of this object; null uses its declared size |
| `image` | `address`, `length` | Virtual address in a loaded section of an executable ELF |

The prepared-object profile requires little-endian RV32 ET_REL/ET_EXEC with at most one each
of SHT_SYMTAB and SHT_DYNSYM; table-free section/range selection is supported; names
such as `.symtab` are not table identity. Dynamic symbol selection does not enable
dynamic loading, TLS or relocation application. Section relocations must reference
the static table through `sh_link`; a different table is an explicit unsupported
profile, never an index interpreted in the static table. A zero-sized symbol needs
an explicit length. A sized symbol cannot be expanded past its declared size.
Overflow, out-of-range, ambiguous or unmapped addresses, compressed sections and
NOBITS are explicit errors. Requests never guess length from the next symbol.
Several ranges share one prepared ELF owner and section metadata; borrowed views
cannot escape its callback.

The `DataExport` returns the object's content identity, the selected bytes
concatenated in range order, and one span per range: selector, section and
section range, file range, image address when known, writability, the SHA-256
of its bytes and its offset into the concatenated bytes.

Writable sections are marked as initialization data, never current runtime
state. Each span reports the section's relocation count,
`overlapping_relocations` for the selected byte range and
`unknown_relocation_extents` for the section. Known fixed-width writes ending at the range start
or starting at its end do not overlap. Unknown transformations cannot establish
nonoverlap from their offset alone. The pinned structural parser currently
supplies RV32 NONE/32/64 classifications; other types retain unknown extents.
Known writes beyond the section are rejected. ET_EXEC relocation sites are
normalized from virtual to section-relative coordinates. No relocation is
applied by the export.

An export has no accepted assertion. It does not generate Rust, publish
register definitions or claim qualification.

## Library register accesses

`register-accesses` analyzes every function of the given libraries in its own
process and reports the memory addresses they access. Inputs are read once and
identified by content, and nothing is retained.

```console
blobray --format json register-accesses --input libphy=/path/to/libphy.a --range 0x20000000:0x9c00
```

Each `--input ROLE=PATH` is an archive or ELF; a function names its input by
position. Every STT_FUNC symbol of a nonempty executable section of every
object is analyzed. Each `--range START:LENGTH` selects resolved addresses that
overlap it; without a range every numeric address is reported, and unresolved
addresses are reported either way. `--working-memory-mib`, `--timeout-secs`
and `--max-work-units` bound the analysis cooperatively.

An unresolved address that the analysis computes as a constant plus a
nonzero multiple of one unknown index (through additions, subtractions,
constant shifts and multiplications and `shNadd`, at most 16 operators deep)
also carries `indexed`: the progression `base + stride * i` modulo 2^32 it
selects from, as an array element chosen at run time. `count` bounds the
index to `0..count` when its expression proves it (a mask, a zero-extending
byte or halfword load, an unsigned remainder, a right shift or a comparison)
and is null otherwise. Two different unknowns, any other operator and a unit
stride with an unbounded index (a pointer plus an offset) carry none. With
`--range`, a progression is kept when its base lies in a range or, for a
bound up to 4096, an index reaches one; otherwise the address stays a plain
unresolved one. The summary counts these as `indexed_addresses`, a subset of
`unresolved_addresses`. Each address of a progression is a may-address: no
index value is claimed to occur.

`--address ADDR` (repeatable) keeps the observations in that 32-bit word and
an indexed observation whose progression may reach it, and
`--function NAME` (repeatable) those of that function; blocked functions it
names, every gap and the summary are never filtered, so a selection cannot
hide an incomplete analysis. `--group-by address|function` groups the selected
observations: one group per word (unresolved addresses first) listing each
function by access kind (a masked expression is `expression`), width and mask
with a count, or one group per
function listing each word the same way. A group's function carries its
symbol identity, so same-named functions of different archive members or
unnamed functions stay apart; the human format shows the member ordinal. An
indexed observation groups with the unresolved addresses and prints its
progression, for example `indexed 0x2010f400+0x10*i (i<6)`, apart from other
progressions. The human format prints the groups
before the summary line and groups by address when a filter is given without
`--group-by`.

A store observation also carries `stored`: the runs of the stored value,
low bits first, each from a fixed value, bits of a register at the function's
entry (an argument for `a0`..`a7`), bits of a loaded value (with `same_word`
for the stored word itself, the bits a read-modify-write keeps) or unknown.
Each bit follows the analysis' expressions through bitwise logic, constant
shifts and additions that cannot carry; anything else is unknown, never
guessed. Each expression node is evaluated once per function, memoized
in working memory reserved before it grows and charged one unit of the work
budget, so shared subexpressions cost no repeated walk and
`--working-memory-mib`, `--max-work-units` and the deadline bound the
evaluation. The human groups print them high bits first, for example
`[31:25]=kept [24:18]=arg0[6:0] [17:0]=kept`, and another word's bits with
their load width (`load 1B 0xa[7:0]`); entries with different sources stay
apart, keyed by the sources themselves.

When at least one stored bit is copied from the same word at the same
position, `write-replacement` covers every other bit of the write. This
includes explicit AND-clears and OR-sets, even when the replacement equals
the hardware reset value. A write without identically preserved bits has no
replacement mask; its `stored` runs still describe the whole write. These
observations establish no hardware RMW safety or register-field meaning.

A load observation carries `reads`: `uses` (`bits`, a tagged `sink`, and
`precision` `exact` or `may`), `unknown` (`bits`, `limit`), `preserved`, and
`unused`. These masks partition the load width with precedence used, unknown,
preserved, unused. Sink sites are instruction offsets in the function's code
address space; `return` instead names a physical result register. Sink tags
are `branch`, `dispatch`, `indirect-target`, `address`, `return`, `store` (with
`address` and `same_word`) and `call-argument` (with `register` and `callee`).
The callee is `null` for an unresolved indirect call; otherwise it contains
`address`, the captured defining `function`, and the symbol `name`, each
nullable when unknown. Relocatable section offsets are not image addresses.
Tail calls use the same sink. Diagnostic-function policy belongs to the checker.
Limit tags
are `partial-function`, `stack-spill`, `unsupported-operation` and `work-limit`.

The read pass follows instruction CFG edges, preserving sources across joins,
known stack stores/reloads and ABI-preserved registers at calls. Differing
mappings at a join become `may`; a source absent on another path is retained
on its carrying paths. Only bits reaching a stopping point are unknown. It
uses admitted state sets, caller work/deadline checks and a per-function
dependency budget of `max(100000, 512 * instruction_count)` units.
See [loaded bit uses](../../../blobray-README.md#loaded-bit-uses).

Human groups show the four classes, sinks, precision and reasons; observations
with different `reads` remain separate entries. Selection masks retain their
expression meaning and do not by themselves indicate a read use.

The JSON document streams `{"schema":9,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"records":[...],"groups":[...],"summary":{...}}`;
`records` holds the selected records and `groups` appears only with
`--group-by`.
`inputs` lists each input's role and SHA-256. A record is an `observation` (the
function, the record ordinal and exact original fact, the address or null, its
alternative index, an unresolved address's `indexed` progression when it has
one and a read-selection or write-replacement mask), a `blocked`
function whose analysis stopped (for example an unknown extent), or a `gap`
naming code no function was selected from: an unsupported object, a thin
archive member, a malformed container or an ELF diagnostic. The summary counts
analyzed, partial and blocked functions, gaps, observations, unresolved and
indexed addresses, and breaks the
partial functions down by cause (`partial_causes`, see
[interfaces and formats](../interfaces-formats/README.md)). `cargo registers check
inventory` runs it with `--abi riscv-integer --call-returns` and compares these accesses,
indexed progressions included, with the register model.

Observations name no hardware registers: the reviewed register model owns
register identity and fields. Instruction access widths remain observations,
not physical register widths. Masks describe analyzed expressions, not proven
physical fields or safe hardware RMW. Finite alternatives are may-addresses;
unresolved values are never discarded. The summary does not promise complete
hardware coverage or absence of register accesses.

The independent [register tool](../../../../../chips/espressif/registers/README.md) owns
source-model initialization, SVD import, reviewed source applicability and
four-output publication. A Blobray observation is research evidence, not an
automatic hardware-source promotion.


## MMIO difference between builds

`mmio-diff` compares two builds of the same firmware (images or libraries) by
their MMIO behavior and reports only its change:

```console
blobray mmio-diff --abi riscv-integer --range 0x20000000:0x1000000 --old image=main.elf --new image=branch.elf
```

Each build is analyzed as `register-accesses` analyzes it (`--old` and
`--new` repeat `ROLE=PATH`; at least one `--range` window is required). A
function's accesses inside the windows are its loads and stores at an exact
address or an indexed progression, each with its width, the bits a loaded
value keeps (`keeps`) and, for a store, where each run of the stored bits
comes from; a run loaded from outside the windows keeps its position but not
its data address. A whole stored word that lies inside a sized function or
data object of a static image (a DMA descriptor, a handler) also names that
symbol's stable name and offset (`0x…(&name+offset)`). Identical accesses
match first; of the rest, a pair whose words lie in the same symbol at the
same offset matches (the object moved between the builds), then a pair
storing the same value whatever symbol it lands in. Functions are matched across builds by their demangled
name without crate disambiguators or legacy hashes, and every instance of a
name is compared as one multiset, so symbol hashes, code layout, branch
offsets and the addresses of loaded data never produce a difference. A
stored word that is an image address no sized symbol contains, such as a
buffer's end, a linker-script bound or an address in part of a word,
compares by its value alone: unchanged code whose layout moves it reports
that store as changed. Only functions
whose accesses differ are reported, with the accesses one build has and the
other lacks (`-` old, `+` new), and a function with MMIO accesses that one
build lacks is all removed or all added. A function either build blocks has
no accesses compared: its state (`analyzed`, `blocked`, `absent` in each
build) is reported where it differs, since its accesses there are unknown.
Each change says whether each build's analysis of the function is complete:
an access missing from an incomplete one may only be unseen. A function
both builds define that is not reported but that a build blocks or analyzes
incompletely is listed as `uncertain`, with its state and completeness in
each build: no difference is seen there, but none is shown absent either. Code a build
does not analyze is reported with its reason (`gaps`), and a build with no
analyzed function, such as a file in no supported format or an image
without function symbols, is an error rather than a build without MMIO
behavior. A function whose accesses outside the stack with an
unresolved address change in number is reported too: there the comparison
sees less, and its accesses there may differ unseen. The JSON document
(`MMIO_DIFF_SCHEMA`) carries both builds' inputs, the windows, the number of
functions with MMIO accesses and of blocked functions in each build, and the
changes.
