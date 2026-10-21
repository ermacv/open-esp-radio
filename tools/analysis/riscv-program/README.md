# Static RV32 images

`oer-analysis-riscv-program` reads one borrowed static RV32 ELF executable, through
[`oer-analysis-elf`](../elf/README.md) only, for the program model of
[`oer-analysis-riscv-model`](../riscv-model/README.md): its ELF
floating-point ABI (`abi`), the `ProgramView` of its memory and the file-backed
constants it serves as `ImageMemory` to the
[analysis](../riscv-analysis/README.md), its code symbols
(`code_symbols`, `code_symbol_at`) and its executable sections. It performs
no loading, relocation or execution, and has no filesystem access.

`facts::ImageFacts` reads, in one parse under the caller's `WorkingMemory`
and `RunControl`, the facts of a static executable that more than one
analysis needs: its functions with their aliases, executable ranges,
read-only words, sized data objects, the address words an `--emit-relocs`
link kept (each checked against the word it relocates) and the compiler's
`.LJTI*` jump-table labels, each table running to the next symbol of its
section. Lookups never parse again. The stack analyzer reads images through
it.

`ImageFacts::address_uses` indexes every non-transfer, non-hint relocation
of allocated code and data, classified by `oer-analysis-elf::rv32`. It keeps
the original site, section, relocation type, target identity, addend and
REL/RELA form, the original symbol (including aliases), the numerical target
and its relationship to this ELF's functions. Exact entries, odd and interior
addresses, overlapping function extents and differing symbol/target
relationships remain distinguishable. Undefined symbols and unsupported
targets are explicit; unknown relocation roles remain indexed. All storage
is admitted in `WorkingMemory` and collection charges `RunControl`.

An address use is a structural observation. It proves neither preservation
of a function-pointer type nor completeness of callback provenance. Stack
analysis projects it according to each consumer's rules for symbol kinds,
section targets, materialization, field types and executor ownership. Each
index belongs to its own ELF; companion section indexes are never looked
up through the main image. The separately checked relocated-word index
continues to supply table contents.

`ProgramView` lays an image's memory out by its sections, as a firmware
packer such as esptool loads it: every allocated section with file contents.
A section's own flags say whether it is code or writable data, whatever the
flags of the load segment holding it, so an ESP-IDF image whose one segment
holds IRAM code and DRAM data (RWE) loads; a reservation without contents
(`NOBITS`, such as `.flash_rodata_dummy`) is no memory and may overlap code.
Each loaded section must still be the bytes a `PT_LOAD` segment carries at
its address, and code must lie in an executable segment, so the bytes
analyzed are the bytes loaded. Loaded sections must not overlap, code must
not be writable, and dynamic loading and runtime relocations are refused. A
thread-local template (`.tdata`) is ordinary data; thread-local storage is
never memory. Constants are the bytes of a section neither executable nor
writable, or of a writable one that a `ReadOnlyWindow` the caller asserts
read-only covers whole (flash mapped for reading, whose sections ESP-IDF flags writable);
code loaded into RAM may hold tables the program rewrites, such as an
interrupt vector table, so it never holds constants. `ImageFacts` takes the
same windows for its read-only words.

`tables::ObjectTables` implements the model's `TableEntries` port for a
relocatable object: a table's entries are the labels its read-only data's
`R_RISCV_32` RELA words name, as many as the dispatch's bounds check states
(a `SHT_REL` table, whose addends live in its words, and a section two RELA
sections relocate name none). `tables::ImageTables` implements it for a
static image over its `ImageFacts`: a labelled `.LJTI*` table is the run of
words the link kept relocated from its label, with or without a bound; a
sized read-only data object is its words; with a bound, the bound's count
of read-only words.

`execution_segments` lends validated static RV32 ELF segment bytes to a caller's
admitted loader. Execution loads the segments themselves, so its profile stays
over them: overlapping mappings, writable code, dynamic loading, thread-local
storage and runtime relocations are refused, and input capacity is admitted. Borrowed bytes expire after the callback; application owns any copied
mutable memory. This port performs no relocation, model selection or execution.

Function analysis uses shared mapping-symbol validation and
admitted interval storage. Only local zero-sized `$d`/`$x` markers authorize data
intervals. ISA-qualified `$xrv32…` markers also end data intervals; other XLENs,
conflicting, misaligned code or out-of-section markers fail closed.
