# Inventory and linked images

Inventory captured executables and link images from their objects, in process.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Inventory

`blobray_application::captured::inventory(executable, memory, control)` reads
one executable given as bytes and returns an `ArtifactInventory`: the container
kind (`elf`, `archive` or `thin-archive`), whether member framing was intact,
and one `ObjectInventory` per object with its `ObjectId`, member name, payload
content identity, ELF sections, symbols and relocations, and diagnostics.

Archive members keep their container ordinal: duplicate names and identical
bytes remain distinct objects of one container content. A thin archive names
its member files instead of holding them; each member is reported with a
`missing-member` diagnostic and no payload, and the caller gives the member
objects as executables of their own. A member whose payload is malformed is a
known member with a diagnostic and no ELF tables. Broken member framing ends
enumeration and reports `malformed-container`: the remaining membership is
unknown and the inventory is incomplete.

Names are native bytes; human rendering can be lossy, JSON byte arrays preserve
them exactly. The inventory charges the caller's control for every member and
record and reserves working memory for what it keeps; exhausting either fails
the call rather than truncating the inventory.

### Symbols and strings

Two read-only commands present the inventory without extracting members or
running external tools on vendor objects:

```console
blobray symbols --input librftest=/path/to/librftest.a --match 'rx_*_gain'
blobray strings --input librftest=/path/to/librftest.a --object phy_test.o
```

`symbols` lists every defined, named function and data symbol with its input,
archive member, section, address, size, kind, binding and citation
`ROLE[member]::name`; with the pinned artifact's id as `ROLE` that is the form
`cargo verification check provenance` prints. `--match` selects names
containing a substring, or matching a whole-name pattern when it contains `*`.
Same-named archive members stay separate rows. `strings` lists every
NUL-terminated run of at least `--min-length` (default 4) text bytes in the
allocated, read-only, non-executable `PROGBITS` sections, with its section
offset; text is printable ASCII, tab, newline, carriage return and bytes above
0x7f, rendered with `\n`, `\t`, `\r`, `\\`, `\"` and `\xNN` escapes so a
non-UTF-8 string stays exact. A run without its terminator is not reported.
Both JSON documents are schema 1 (`{"schema":1,"inputs":[...],"symbols":[...]}`,
`{"schema":1,"inputs":[...],"strings":[...]}`).

## Synthetic linked images

`blobray_application::linking::link(request, executables, linker, host,
directory, memory, control)` links the image a `LinkRequest` describes and
returns a `LinkedImage`: its `ImageManifest`, the ELF bytes as an `Executable`
and the linker's map. `linker` is the path of an explicitly selected LLD or GNU
BFD ld; `host` is the linker adapter (`blobray_linker::ElfLinker`);
`directory` is where the link's private workspace is created and removed.

A `LinkRequest` has ordered `inputs` (executable content identities), the
`entry` and optional additional `roots` (`SymbolId`s), `companions` (symbols of
companion executables, see [explicit ROM companions](../analysis/README.md#explicit-rom-companions)),
the `layout` and the names deliberately left `absent`. Obtain a complete
physical `SymbolId` from an inventory; names are not selectors. The layout has
`code` and `data`, each with numeric `start` and `length`. Regions must be
nonempty, start on 4096-byte boundaries, fit RV32 and not overlap. A request
takes 1–512 inputs and at most 63 additional roots.

`linking::propose_companions` links the same request as a trial with
unresolved names permitted and resolves each unresolved name against up to 16
candidate executables; see [contracts](../../../docs/design/contracts.md#image-linking).
It never keeps an image.

### Link policy and evidence

The link accepts little-endian RV32 ILP32/ILP32F/ILP32D relocatable objects and
ordinary archives. Companion code and data participate in the same link. Every
selected payload must be supported; malformed, missing, TLS, RV32E and
quad-float inputs block the link. Linked firmware/ROM ELF inputs, dynamic
linking, arbitrary scripts and caller-supplied linker flags are not supported.
Root symbols must be defined global/weak executable symbols with unambiguous
static-table and section identity. Input names containing CR/LF are rejected
because the linkers' unescaped maps cannot safely represent them.

Each member is materialized once as `i<input>-m<ordinal>.o`; duplicate archive
names and byte-identical members remain distinct occurrences. Root objects are
forced first in entry/root order, deduplicated by occurrence and removed from
their original lazy groups. Remaining archive members use ordered
`--start-lib`/`--end-lib` groups; standalone objects are direct inputs. This
explicit synthetic selection policy can differ from the producer's original
link and never establishes original firmware selection.

The generated script defines CODE and DATA regions, RX/RW load segments,
text/rodata/eh-frame, data/sdata and bss/common sections and the RISC-V global
pointer. Both adapters use `elf32lriscv`, section GC, emitted relocations,
no relaxation, no build ID, no demangling and strict undefined-symbol handling.
LLD also explicitly disables ICF and uses one thread. The environment contains
only `LC_ALL=C` and `TZ=UTC`; core dumps are disabled. Other sections follow the
selected tool's placement rules and must pass the same post-link validation.
No PATH/rustc search, archive regrouping or linker substitution occurs.

`ElfLinker` recognizes the family using `--version` and exercises the actual
adapter with two bounded synthetic RV32 links before accepting it. Their ELF,
raw evidence, normalized observations and stderr digests must agree. Each probe
output channel is capped at 64 KiB and observations at 128 records. The probe checks
root extent/layout, garbage collection, retained relocations and normalized
placement/extraction evidence through the production validators. Version numbers
are not an allowlist: missing `elf32lriscv`, `--start-lib` or required evidence
rejects that executable with `Incompatible`. The version's first line and the
executable's SHA-256 form the linker identity, and the executable's hash is
checked again before and after linking. This does not snapshot dynamically
loaded system libraries.

GNU ld processes archives in order; LLD can satisfy backward references from
previous archives. Both implement `static-analysis-elf-link-v1`, but their
selected members, success/failure and images can differ. Blobray preserves
these differences and never claims original firmware selection.

Validation requires RV32 ET_EXEC, bounded nonoverlapping PT_LOAD segments inside
the declared regions, no writable executable segment, allocated sections covered
by load segments, file-backed executable roots and no unresolved symbol relocation
in allocated sections, including unresolved weak references. The selected entry
must equal `e_entry`. Empty PT_LOAD records are retained without mapping memory;
nonzero file bytes still require a sufficient memory extent. Root addresses are
proved by normalized input-section placements plus source symbol offsets,
unchanged section sizes and matching output symbols. Hidden symbols may be
localized by LLD. Symbol rows cannot impersonate section rows. No exact root
mapping means no image. Other map records keep their reported source occurrence
with `exact: false`; they are observations, not a complete
instruction-by-instruction provenance proof. Every blocker the link finds, up
to 32, is joined into one `link-blocked` error.

The `ImageManifest` (schema 4) records the request, linker contract and
identity, ABI, the ELF's content identity, entry, resolved roots, segments,
source mappings and the successful linker's exit and last 8192 stderr bytes.
The manifest labels the image synthetic.

### Ownership and limits

Domain owns the request, manifest and typed observations; artifacts owns input
and output ELF validation. Application owns selection, materialization, semantic
policy, root proof and validation of every linker claim. The Linux adapters own
CLI/script dialects and parsers behind `LinkerHost`; the common subprocess
helper owns bounded transport and the linker process, which is killed and
reaped when the link ends early. `SectionPlacement` and `ArchiveExtraction`
identify the materialized member; application checks occurrence identity,
extraction coverage, root extent and exit observations. LLD extraction evidence
comes from why-extract; GNU extraction comes from the map.

LLD ELF, map and extraction output flow through concurrently drained pipes. GNU
needs a seekable ELF: the link bounds its extent by the working capacity left
after the fixed 8 MiB link metadata, and the adapter enforces that ceiling with
a hard `RLIMIT_FSIZE`. The metadata reservation covers up to 512 inputs, 4096
members, 64 roots, 32 blockers and the bounded capability probe. Root names are
at most 512 bytes, map/observation records 64 KiB and the stderr tail 8192
bytes. Capacity exhaustion fails explicitly. The workspace is a private
temporary directory below the caller's directory, removed when the link ends.

Real-link tests require both LLD (reference 22.1.8) and GNU ld (reference
2.47 with RV32; `--start-lib`/`--end-lib` are new in 2.47), installed on the
host. The LLD the pinned Rust toolchain ships (`gcc-ld/ld.lld`) and a GNU ld
2.47 build pass them; Ubuntu's LLD 18 and binutils 2.42 fail these tests. Set `BLOBRAY_TEST_LLD` to override
`/usr/bin/ld.lld`. `BLOBRAY_TEST_GNU_LD` selects the GNU executable; without it,
tests use the first of `riscv-none-elf-ld`, `riscv32-unknown-elf-ld`, `riscv32-esp-elf-ld`,
`riscv64-unknown-elf-ld`, `riscv64-elf-ld` and `riscv64-linux-gnu-ld` on `PATH`.
The adapter always selects `elf32lriscv` and passes archive inputs with
`--start-lib`/`--end-lib`, so any RISC-V cross GNU ld from binutils 2.47 or later
qualifies. Earlier releases, including ESP-IDF's binutils 2.46 toolchain, and a
GNU ld with only x86 emulations fail the capability probe. Missing tools fail
tests. Tests use synthetic RV32 inputs, not hardware qualification.
