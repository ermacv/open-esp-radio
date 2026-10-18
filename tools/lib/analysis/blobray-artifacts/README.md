# Blobray artifacts

`oer-tool-analysis-blobray-artifacts` owns structural ELF/AR inventory over borrowed, already
captured bytes. Its only internal dependency is
[domain](../domain/README.md). It never resolves a filesystem path, acquires a
writer, decodes instructions or selects a linker definition.

`MemberCursor` enumerates physical payload ordinals from the domain `ByteSource`
port. It reads metadata and names on demand, returning a scoped name and a payload
range or an external-member request. GNU/BSD/COFF metadata and AIX index entries
never consume payload ordinals; AIX traversal follows the finite index rather
than input-controlled links. Cursor framing errors terminate enumeration; the
application records unknown remaining membership. Resource errors abort the run.

`inspect_source` hashes a payload with bounded reads. Unknown formats need only a
prefix; ELF32/64 parsing requires one admitted contiguous object buffer and name
workspace. `inspect_payload` also supports caller-owned bytes. Both require
explicit `WorkingMemory` and `RunControl` ports. They emit sections, symbols,
relocations and diagnostics synchronously through `ElfSink`, returning only ELF
header metadata. There is no container-wide inventory or symbol-set accumulator.

A `ReadRef` adapter bounds ELF delimiter scans; table visits, name copies and
range reads participate in cooperative work accounting. Sink failures preserve
their original error and cannot become malformed-object diagnostics. The caller
owns any retained record copies and their memory capacity. Borrowed views cannot
outlive the admitted input buffer. See the
[memory boundary](../../cli/reference/resources/README.md#current-memory-boundary) for admission
accounting and the single-ELF size limit.

ELF inspection reads raw section/symbol/relocation tables, including null and
local symbols and malformed-name entries. Table kind, section and entry index
remain part of symbol identity. The
[implemented scope](../../cli/reference/interfaces-formats/README.md#identity-and-schema-1) defines supported
formats and coverage limits. No inference here establishes linking, target
execution support or verification success.

[Image validation](src/image.rs) checks captured link inputs and prepared RV32
ELFs using admitted object buffers. It rejects unsupported ABI/TLS, ambiguous
roots, invalid segment placement and unresolved allocated relocations. Application
supplies exact map-derived root addresses; artifacts checks the output ELF against
them and never chooses a different source definition. Successful validation proves
the synthetic image contract, not firmware placement or execution readiness.

[Function views](src/function.rs) lend one admitted captured ELF object's selected
code range selected by physical symbol or explicit object/section/extent, physical relocation targets and ELF data/code mapping ranges to a
callback. The borrowed view cannot outlive its reservations. A missing symbol
extent requires an explicit caller range; this module does not infer function
boundaries or interpret ISA relocations. Reserved null symbols remain explicit
structural records instead of fabricated external definitions.

Static ET_EXEC functions use image virtual addresses and a borrowed `ImageMemory`
view over the image's loaded sections, each the bytes a load segment carries
([static RV32 images](../../../riscv/program/README.md)). Input buffers and the
section table retain working-capacity reservations until the consumer returns.
Sections neither writable nor executable provide constants, and writable ones
inside the caller's read-only windows (`with_prepared_object`'s `read_only`);
other writable, executable, unmapped and zero-fill ranges cannot.
ELF ILP32/ILP32F/ILP32D ABI is recorded independently of ISA semantic coverage.
Static relocation metadata is retained without reapplying edits to linked bytes.


Static RV32 image views, `execution_segments`, `executable_sections`, code
symbols and the `$d`/`$x` mapping-symbol validation belong to
[`oer-tool-analysis-riscv-program`](../../../riscv/program/README.md); function views and
image validation use them.

`with_prepared_object` owns one captured buffer, ELF/program view and lazily prepared sections. Callback-borrowed function views share target names and section metadata; object scope releases their admitted capacity. `MemberIndex` traverses an archive once and preserves ordinal payload identity, including thin-member markers.

An ordinal index retains valid prefix entries when a later archive header is malformed and exposes the terminal integrity error. Selecting an unavailable later ordinal returns that error. Inventory coverage owns the incomplete-membership claim; an index never asserts archive completeness. Resource, I/O and cancellation failures abort indexing.

`PreparedObject::with_data` lends exact file-backed section, symbol or executable
VMA ranges from the same verified buffer and prepared sections as function views.
It preserves raw relocations and writable-initialization metadata, rejects missing
or ambiguous backing, and gives no runtime-value or semantic-layout guarantee.

Prepared data/function selection validates SHT_SYMTAB/SHT_DYNSYM kind, physical
section and entry index without a name lookup. At most one of each table kind is
supported. A selected dynamic symbol never changes the table used to interpret
static relocations: their targets retain the `sh_link` table identity. Relocations
using another table are explicitly unsupported by this static profile. Mapping
markers from both tables share the same validation and admitted interval owner.

A function view is the declared extent of one physical symbol. A zero-sized
symbol has no extent and fails with `needs-extent`. Empty, unaligned,
overflowing, non-executable and unbacked extents fail; no neighboring symbol is
consulted.

Data views expose their relocation coordinate space beside the borrowed
records, from which data export counts the relocations overlapping a range;
artifacts neither applies RISC-V relocations nor chooses targets.

`PreparedObject::executable_bytes` lends a bounded 2/4-byte prefix from an executable
loaded section. ISA consumers validate instruction meaning through their
decoder; artifact acquisition never supplies branch semantics.

`inspect_link_definition` lends no program view: it validates a standalone static
function symbol against captured RV32 ET_EXEC bytes and returns its address.
The extent must have one executable, file-backed load mapping agreeing with the
allocated code section. Non-executable mappings may overlap that address; they
do not identify the selected function bytes. TLS/dynamic metadata or unrelated
overlapping executable mappings are outside this acquisition's authority. Execution/function analysis still
reject unsupported whole-image profiles. Hashing and traversal consume the run
budget; admitted carrier bytes are released before returning the address.
