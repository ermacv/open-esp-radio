# Reviewed register publication

`oer-registers-generate` validates source-owned hardware descriptions and generates
SVD, raw PAC, restricted PAC API with its typed register blocks and the binding
index.

`cargo registers check inventory --chip <chip>` checks vendor uses of register
bits against the source model. It calls Blobray's `library::register_accesses`
over every function of every pinned vendor binary, excluding firmware outputs,
inside the publication's owned MMIO ranges, with `--abi riscv-integer
--call-summaries`. Each typed record is folded into compared words as it
arrives, without a child process or a `registers.json` corpus. It requires a
complete `reads: ReadUses`
classification for each integer or floating load, using the shared
`oer-analysis-blobray-domain` types. A `CallArgument` to an exact name in the
checker's [chip-specific cited diagnostic lists](src/checks/inventory/uses/read_policy.rs)
does not count as use. Other sinks, including unresolved calls and behavior
parameters such as `ets_delay_us`, count with both `Exact` and `May` precision;
read use comes only from load dependencies. Diagnostic evidence is
retained, and never hides another use or an unknown observation of the same bit.

Stores count explicit `write-replacement` bits and constant bits that differ
from the inherited known reset. Same-position same-word preservation and
whole-word reset writes are not uses. Unknown reset or stored value remains
unknown unless an explicit replacement mask establishes use.

The check writes `target/register-inventory/<chip>/report.txt` and `report.json`
before failing on used bits without fields. The JSON envelope is schema 1;
`analysis_resources` records the `register-inventory` profile, effective limits,
work and admitted-memory usage, and typed affected events, functions and records.
Counters are merged across disjoint inputs while call summaries still see the
whole corpus. `inventory-enumerated-indices` and `declared-word-runs` remain typed
unaccounted limits. `reserved_violation_bits` are
uses of a source's explicitly reserved bits, with per-source masks;
`absent_violation_bits` are uses of bits without a field or reserved annotation.
Both are errors. The single register property `reserved = { mask = ...,
sources = [...] }` is validated by the native model and retained outside SVD;
it never creates PAC fields. `may_only_bits` lists each word's
uses supported only by `May`, excluding any exact read or proven write use.
Read accounting ranks callees by source bits, retaining targets and diagnostic
SOURCE citations, and reports diagnostic-call and diagnostic-only shares.
Each load's source bits are unioned across sites/arguments before indexed
address expansion; callee groups can overlap. Resolved/indexed MMIO and all
observations including unresolved addresses have separate totals. Unknown bits
are listed separately by word, with their reasons and source symbols, and do not create opaque fields
or establish absence. The reports retain input hashes, assumptions, ranges,
partial/blocked/gap counts and causes, and unresolved/indexed address limits.
Access counts and the provenance registry's cited-function priority are retained;
a citation alone is not evidence that any particular bit is used.
Small bounded indices compare every candidate; larger or unbounded progressions
compare declared arrays and runs and retain a coverage limit. The report
retains the summary returned by the same analysis and a `cargo blobray`
command that reproduces its corpus as JSON for manual inspection.
The MMIO corpus does not cover RAM layouts. No model is edited by this check.

`cargo registers init-model --request geometry.toml --directory new-model`
creates an editable native model from an explicit schema-1 TOML request:

```toml
schema = 1
chip = "example-chip"
address-space = "cpu"
[device]
name = "EXAMPLE"
version = "1"
description = "Unreviewed source model"
address-unit-bits = 8
width = 32
[[peripherals]]
name = "CONTROL"
base = 0x20000
length = 256
```

The new model has empty peripherals and address blocks, no inferred register
widths or fields. Device metadata/defaults use `ModelDevice`; the device bus width
does not declare the size of any register. Add explicit reviewed source assertions
with physical chip/address-space/address/width identities and evidence before
publication. Binary access observations support investigation; they do not supply
hardware access or write semantics.

`cargo registers import-svd --source device.svd --directory new-model --chip
example-chip --address-space cpu` imports standard CMSIS-SVD declarations into the
same schema-3 manifest/schema-2 fragment format. It preserves original XML as
`source.svd`, including extensions outside the native model. Imported declarations
have no review annotations or accepted assertions. The initializer similarly
retains `initialization.toml`. `device.toml` is the model entry point.

Both commands require a new directory with an existing parent. They validate and
render before exclusive directory creation, sync the source/fragment and write
the manifest last. Errors clean up that invocation's directory; an abrupt process
death can leave an incomplete draft. Existing destinations are never overwritten.
These operations create source models only; applicability, review/evidence packs
and publication policy are supplied explicitly through the following manifest.

```console
cargo registers validate --manifest chips/esp32s31/registers/publication/registers.toml
cargo registers generate --manifest chips/esp32s31/registers/publication/registers.toml --check
```

Omit `--check` to publish an intentional reviewed source change. `--check` is
read-only and fails if any selected output is absent or differs. All four outputs
are prepared before replacement; each file is replaced atomically, but a failure
between replacements reports an incomplete batch. Rerun generation to finish;
there is no claim of atomicity across four directories.

A device manifest may also place layouts of a register library through
`[[shared-fragments]]` entries (library manifest, layout fragment, the chip's
review file and a base address per peripheral). A library layout has no
`baseAddress` and no reviews; the loader rejects either, and a placement must
name exactly the layout's peripherals. A publication manifest that names a
`library` instead of a `model` publishes that library: the typed renderer
renders every layout as a typed register block without an address (its owner
is generic over its placement, its constructor takes a `Block` at any fixed
base) with the library API's transactions as its methods, into the library
crate's `generated.rs`. A library API lists no typed or unexposed
peripherals, ownership partitions, register sides or interrupt sources; the
tool writes no SVD or binding index, since a library has no addresses. A
chip that places a library layout lists it in its `typed-peripherals`; its
`generated.rs` re-exports the library's module with the chip's `BASE`,
`Claim` and an owner alias at that base, and the chip's policy may not define
transactions, register sides, write-trigger fields or FIFO ports on that
peripheral. See [`chips/espressif/registers/ieee80211`](../../../chips/espressif/registers/ieee80211/README.md).

## Typed register blocks

A PAC API policy's `[options] typed-peripherals` lists the peripherals the
generator renders itself, as typed register blocks over `oer-mmio`
([`portable/mmio`](../../../portable/mmio/src/lib.rs)), into the PAC API
output (`generated.rs`, which starts with `#![forbid(unsafe_code)]`).
`[options] unexposed-peripherals` lists SVD peripherals kept only for
bit-use coverage: they belong to no partition and neither backend renders
them. svd2rust renders the other peripherals from the published SVD with
the typed and unexposed ones cut out; the typed ones' transactions, pointer
fields, register-bound domains and partition members leave its policy. A
publication whose PAC names no `raw` crate (and no `crate-name`) has every
peripheral typed or unexposed.

A typed block is a module named after the peripheral with `BASE`, `SIZE`
(the bytes to the end of its last word), `BlockWords` (its declared words:
the block's whole register authority), its `Claim` type and an owner type
generic over its placement: device registers (`new(Claim)`) or host memory
(`host(&mut [u32])`, for tests of the accessors). Per register the owner
has `read_<register>` (an `R` value with a getter per field),
`modify_<register>` (a read-modify-write through a `W` that keeps every bit
no setter changes) or, for a write-only register, `write_<register>` (a `W`
from zero; a write-only write-one-to-clear register is written so too). A
readable write-one-to-clear register has `sample_<register>`, an affine
`Snapshot` of its declared fields that `retain_<field>` narrows, and
`acknowledge_<register>`, which writes exactly that snapshot. An array
takes an `oer_mmio::Index<N>`. Each register module holds `FIELDS`,
`OFFSET` and a value enum per enumerated field (`<field>_variant` reads it).
A field setter takes `bool`, the value enum, the exact native integer (8,
16, 32 bits) or `oer_mmio::Bits<WIDTH>`, and its getter returns the same
type; a read-only field and a DMA address field have no setter. A register
changed by its read is read through `&mut`; a cluster, a field array, a
register that is not 32 bits wide, mixed write-one-to-clear and ordinary
fields in a readable register, a write-only field in a readable register
that the policy does not declare a write trigger and a write range narrower
than the field are refused, except the range of zero alone: such a field
(a reserved lane the vendor always writes zero) has no getter and no
setter, its register module holds `ZERO`, and every write of the register
leaves it zero.

`[[pointer-fields]]` on a typed peripheral (`register`, `field`,
`points-to` a DMA layout, optional `member-offset` and `encoding`) makes the
field a DMA address: its register module holds `<Field>Image`, made only
from a `DmaRef` to the layout (`new`, or `NULL`), and the field's setter
takes that image, never an integer. The image holds the whole address (a
32-bit field), its low bits (`encoding = "low-bits"`, any narrower width:
a `const` assertion checks that every address of the DMA window shares the
high bits the hardware supplies) or the BLE word index (`ble-compressed`, a
20-bit field; `new` returns `None` for an address without one). A
zero-based field write takes the image as its argument; no other
transaction, fixed image or masked modify sets a bit of a DMA address
field.

`[[interrupt-snapshots]]` on a typed peripheral pairs a status register
read without a side effect with the write-one-to-clear register that
acknowledges it: `sample_<name>` returns an affine `<Name>Snapshot` of the
status fields the clear register declares (`EVENTS`; a status field
acknowledged only in part is refused), `retain_<field>` narrows it, and
`acknowledge_<name>` writes exactly the snapshot to the clear register.
Both methods belong to every owner that owns the clear register and reads
the status register; the clear register has no other writer, and no
transaction names it.

`[[write-trigger-fields]]` (`peripheral`, `register`, `field`, `sources`)
declares a one-bit field whose written one starts a hardware action (an
update, a feed, a reset or a done strobe). The field has no getter and no
setter; the register module holds `TRIGGERS`, every read-modify-write of
the register (`modify_<register>` and each policy transaction) writes zero
there, and only `trigger_<register>_<field>` writes one: the value read with
every trigger zero and this field one, or, in a write-only register, this
field alone. `[[fifo-ports]]` (`peripheral`, `register`, `sources`)
declares a register of one field whose read takes an element out:
`pop_<register>(&mut self)` returns the field and `push_<register>` writes
it from zero. A FIFO port has no `read_`, `modify_` or `write_`, no side
observes it, no policy transaction names it, and its SVD read action does
not refuse it. Registers are 32-bit words, never bytes: a CLIC line word
holds its pending, enable, attribute and control bytes, so changing its
enable or control writes back the pending bit read. That is sound only
while every vectored line is level-triggered, and a CLIC block's model
description records that invariant.

`[[shared-blocks]]` (`peripheral`, `description`, `sources`) declares a
typed block whose one owner every execution context reaches at once: a
counter latch and its value, a software interrupt line. Its registers are
side-effect-free reads, registers whose writable fields are all write
triggers, and registers of one field stored whole; any other register, a
split or a policy transaction on it is refused. Every method takes `&self`:
`read_<register>`, `trigger_<register>_<field>` (the trigger alone, zero in
every other bit, nothing read back) and `store_<register>` (the one field
from zero). Its `BlockWords` implement `oer_mmio::SharedRegisters`, so the
owner at its fixed address is `Sync` and lives in a `static`.

The policy's `[[ownership-partitions]]` of typed peripherals become
`peripheral_ownership`: a struct per partition holding its owners,
`PeripheralPartitions`, `Claims` (every typed block, as nested tuples the
PAC root claims with one `oer_mmio::assume_owned`) and `partition`, which
gives each claimed block to its owner. A `const` assertion beside them
checks that no two typed blocks list the same word. A partition marked
`handoff = true` is claimed by its own platform handoff instead: it is left
out of `PeripheralPartitions` and `Claims` and has `<Name>Claims` and
`<Name>::from_claims`. A partition mixing typed
and untyped peripherals is refused.

`[[register-sides]]` divides one typed block between its interrupt
handler's side, `<Block>Irq`, which owns `irq` and observes (reads only)
`irq-observes` (registers without a read side effect), and its task side,
`<Block>Task`, which owns every other register. The block's `split` and
`join` move between the whole owner and the two; `oer-mmio` checks at
compile time that the owned word lists are disjoint, and a generated
`const` assertion that every word of the block belongs to exactly one side.
`[[interrupt-sources]]` gives a typed block's interrupt-matrix source
number, rendered as its `SOURCE` constant with the evidence sources.

`[[read-capabilities]]` (`peripheral`, `name`, `registers`, `copy`,
`description`, `sources`) gives the reads of some registers of a block to
`<Block><name>`, an owner over `oer_mmio::Reader` that has only their
readers; each reads without a side effect. Naming every register, the
block becomes it (`into_<name>`); otherwise the block splits into it and
`<Block>Rest`, the owner of every other register (`split_<name>`). Nothing
writes those registers afterwards, so with `copy = true` the reader is
`Copy`, and over device registers `Send` and `Sync`.

`[[register-divisions]]` (`peripheral`, `description`, `sources` and two to
four `parts`) divides a block for good between owners `<Block><part>`,
which its `divide` makes. A part owns whole `registers`, a range of array
`elements` (`register`, `first`, `count`: its own array, indexed from
zero), `observes` registers read without a side effect and has its own
bits of shared write-only trigger words (`triggers`: `register` and its
declared write-trigger `fields`). A trigger word's parts store only their
own bits, zero in every other one, which a written zero leaves alone:
`trigger_<register>_<field>`, and each fixed image or sequence step that
sets only the part's bits. `oer-mmio` checks at compile time that no two
parts own a word or a trigger bit and that no part owns a trigger word.
A block takes at most one of register sides, a read capability and a
division.

The policy's transactions on a typed peripheral become methods of every
owner that owns their register (a read: owns or observes):
`full-register-writes`, `full-register-reads` (opaque domain),
`fixed-register-images`, `fixed-register-sequences`,
`masked-register-modifies`, `field-or-modifies`, `field-clear-modifies`,
`field-replace-modifies`, `zero-based-field-writes` and
`sampled-bit-zero-writes` (one read, then the field as read and zero in
every other bit). Their masks are asserted against the register's declared
fields in `const` items beside them, except a fixed image or a sequence
step marked `whole-word = true`: the vendor's store of a whole word, with
its evidence sources, which also writes the bits the model leaves
undeclared. The other transaction kinds are refused on a typed peripheral
(a field read is the register's typed reader, a store of zero is the fixed
image zero); a W1C register needs no `w1c-register-snapshots` entry. A
`partition-image-reads` entry on a typed partition becomes the
partition's method of that name and its `<NAME>_LEN`: it reads every
readable register of the partition once, by index in ascending address
order, through each block's `read_image_word`; a register whose read
changes it is refused. A bounded domain whose
range is every value of a field width also takes that field's value read
back, `from_bits(oer_mmio::Bits<WIDTH>)`.

## Peripheral review defaults

Schema-2 peripheral fragments and schema-1 chip review files for shared
peripherals may carry separate `[[peripheral-review]]` tables. Each table's
`entity` names one peripheral declared in that fragment (or placed from the
shared library). It applies to that peripheral alone, including when a file
declares several peripherals:

```toml
[[peripheral-review]]
entity = "RADIO"
opaque-entities = ["RADIO.CONTROL.BITS_1_31_OPAQUE"]

[peripheral-review.peripheral]
sources = ["radio-evidence"]
provenance = "derived"
accuracy = "exact"
completeness = "partial"

[peripheral-review.register]
sources = ["radio-evidence"]
provenance = "observed"
accuracy = "exact"
completeness = "partial"
naming = "descriptive"

[peripheral-review.field]
sources = ["radio-evidence"]
provenance = "observed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[peripheral-review.opaque]
sources = ["radio-evidence"]
provenance = "observed"
accuracy = "positional"
completeness = "partial"
naming = "opaque"
```

The `register` template also covers clusters and reusable `dim` declarations;
review paths use template names, including `%s`, rather than expanded instances.
`naming` still applies only to registers and fields: a cluster can use an
explicit review without `naming` when its register template includes that key.
Interrupt and enumeration-value records remain explicit.

`opaque-entities` explicitly selects individual registers or fields by their
full review paths. An `_OPAQUE` suffix does not select a template, and marking
a register does not mark its fields. A listed entity uses the independent
`opaque` template when supplied, otherwise its level's template. This does not
merge metadata between templates.

An explicit `[[review]]` replaces the applicable template in full, including
omitted keys. Each peripheral, cluster, register and field in a declared scope
must have a resolved review.
An empty template supplies an empty record, making no evidence or naming
claim. Existing validation of naming, sources, classification and reviewed
coverage runs on resolved records; defaults cannot turn a navigation hint
into accepted evidence. Unknown keys, absent entities, duplicate scopes,
duplicate opaque selections, duplicate reviews, and an explicit review equal
to its applicable default are errors. There are no global or cross-file
defaults.

The loader resolves templates before any consumer reads reviews, using the
same resolver as memory layouts. These source tables live outside SVD
`[[peripherals]]`; SVD imports create no review templates, exports carry no
template syntax, and shared library layouts cannot contain chip reviews.

## Memory layouts

A device manifest's `layouts` lists memory layouts: structures in RAM whose
format the hardware defines ([hardware memory](../../../docs/hardware-memory.md)).
Each schema-1 file holds one `[layout]` with `name`, `description`, `size`,
`align` and `placement` (RAM regions of `memory.toml`), and either
`[[layout.words]]` (`access = "shared"`, every 32-bit word declared, in
offset order) or `[[layout.bytes]]` (`access = "owned"`, ranges of one, two
or four little-endian bytes). A word or range has `fields` with `bits`
(`"N"` or `"HIGH:LOW"`), `writer` (`cpu`, `hardware` or `phased`) and an optional
`description`; a whole-word address field names either the `placement` of
the buffer it points at or, in `points-to`, the layout it links to. Typed
addresses exist for the `dma-window` region only: such a field's placement,
and the placement of the layout it points to, is `["dma-window"]`. Fields do
not overlap. A layout
that repeats another's geometry names it in `same-geometry-as`; the tool
checks that size, alignment, placement, words and the bits of every field
both name agree (the RX and TX DMA descriptors).

`writer = "phased"` requires a nonempty ordered `phases` list of
`{ name, writer }` entries. Names are unique and each phase's writer is
`cpu` or `hardware`. Single-writer fields cannot declare phases. These
declarations document handover; they do not implement its protocol.

A 20-bit pointer field can name `points-to` with
`encoding = "ble-compressed"`: a nonzero word index with a reconstructed
`0x2f` SRAM prefix. Its builder accepts `Option<DmaRef<L>>` and returns
`None` for an unencodable reference; clearing the pointer preserves other
fields of the word. Its decoded-address getter grants no reference to
hardware-written memory.

A compressed pointer to a byte buffer names `placement = ["dma-window"]`
instead of `points-to`. Its builder takes `DmaWindowAddress`, checks the
same nonzero word-index encoding and preserves other fields. The buffer
may begin with an owned RX/TX prefix; that decoder is not the pointer's
target type. `without_<field>` clears this nullable buffer address.

Instead of words or byte ranges, a fixed array declares `[layout.array]`
with `element = "<LAYOUT>"` and `count`. Its element is a shared-word or
owned-byte layout, with the same alignment and placement as the array and a
size whose product with count is the array size. Nested arrays are rejected.
A shared-word array exposes checked `entry(index)` references and is itself
a typed pointer target. An owned-byte array is a borrowed decoder over a
fixed-size byte image: `SIZE`, `COUNT`, `new`, `bytes` and checked
`entry(index)` returning an element decoder. When the element has CPU-written
fields, its array writer exposes `view` and checked `entry(index)` borrowing
one element writer. Out-of-range indices, including `usize::MAX`, return
`None`. Byte arrays remain buffers, never `DmaRef` targets.
The channel-frequency lookup uses the indexed form; the packet-start and
RX-address-delay lookups keep their named PHY-mode slots.

Its `[[review]]` entries follow the register rules: for publication the
layout, every word or range and every field carry sources, provenance (not
a hint), accuracy and completeness, and every word, range and field a
`naming`. An opaque word is `WORD_<offset>_OPAQUE`, a range
`BYTE_<offset>_OPAQUE`, a field `BIT_<n>_OPAQUE` or `BITS_<low>_<high>_OPAQUE`.
Review sources must exist in the evidence catalogs.


A layout can declare `[layout.review]` as the default review for its own
identity, words or byte ranges, and fields. The keys are `sources`,
`provenance`, `accuracy`, `completeness` and `naming`. A separate
`[layout.review.opaque]` supplies the complete default for an explicitly
opaque word, range or field:

```toml
[layout.review]
sources = ["REVIEWED_LAYOUT_SOURCE"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[layout.review.opaque]
sources = ["REVIEWED_LAYOUT_SOURCE"]
provenance = "reviewed"
accuracy = "positional"
completeness = "unknown"
naming = "opaque"
```

An entity opts into the opaque default with `opaque = true` on its word,
byte-range or field declaration. This selects that entity's review only;
it does not infer opaque status from a name or mark its children. Opaque
names still have to match their position. The opaque default is independent
of the ordinary default; its keys are not merged with that table.

An explicit `[[review]]` replaces the applicable default in full, including
omitted keys. Defaults belong to this layout alone. Publication still
requires a complete resolved review for every entity. Unknown keys,
reviews for nonexistent entities, duplicate reviews and an explicit review
identical to its applicable default are errors.

A layout has no address: it is not in the SVD or the binding index. The PAC
API output gains a `layouts` module: a constant per named RAM region; per
word layout a `#[repr(C, align(N))]` type of `oer_mmio::VolatileCell` words
with `zeroed`, `SIZE`, `ALIGN`, `PLACEMENT` and a derived
`bytemuck::Zeroable`, each cell with `read`
and `write` of a value type, which has `from_bits`, `bits`, a getter per
field and a `with_*` builder (`Option<Self>` when the value may not fit);
per byte layout a decoder over `&[u8; N]` with a value type per range and a
getter per field; a byte layout with CPU-written fields also gains a writer
over `&mut [u8; N]` (`set_<range>` and the range getter) and `from_bits` and
`with_*` builders on the ranges the CPU writes, never on a hardware-written
field. When the memory map has a `dma-window` RAM region it also gains the typed
addresses of the DMA region, `oer-mmio`'s bound to the chip's `DmaWindow`:
`DmaWindowAddress` for a buffer and `DmaRef<L>` for a layout placed there
(`DmaLayout`), both with an `unsafe` constructor in `oer-mmio` that only the
chip HAL's `dma_address` module calls. A model with BLE-compressed pointers
also gains `ble_word_index`, the const encoder of an address. An address
field's builder takes that type (`Option<DmaRef<L>>` for a link, `None` for
none); its getter returns the word as read. No ordering, fence or ownership
is generated.

A chip publication may name `[outputs.linker]`: `path` and
`address-low-field = "PERIPHERAL.REGISTER.FIELD"`. It writes a linker
fragment with `DMA_WINDOW_ORIGIN` and `DMA_WINDOW_LENGTH` (the memory map's
`dma-window` region) and `DMA_ADDRESS_LOW_BITS` (that field's width, from
bit 0, up to the complete 32-bit address), which the runtime linker script includes.

A complete-register write in the API policy takes a `domain`, or, for a
register the hardware reads as a pointer, `points-to = "<LAYOUT>"` (its
bridge then takes a `DmaRef` to that layout of the model) or
`placement = "dma-window"` for a buffer address (a `DmaWindowAddress`).

For a pointer field inside a composed transaction, `[[pointer-fields]]`
declares `name`, `peripheral`, `register`, `field`, `points-to`, optional
`encoding = "ble-compressed"` and `sources`. The generator validates the
field width against the SVD and the target against DMA layouts, then emits
a typed `Option<DmaRef<L>> -> Option<u32>` image encoder; the existing
transaction retains its write/RMW and fence order. A `member-offset` names
a byte position inside the referenced layout, for a register that consumes
a member address. It must fit the layout and, for BLE compression, be word
aligned. Null stays zero; invalid encodings and address overflow return
`None`. No observed hardware address creates a `DmaRef`.

The schema-1 manifest selects model, sparse reviewed assertions, applicability,
memory map, ownership policy, API/lint packs, evidence catalogs and output paths.
Paths are relative to that manifest. No configuration is inferred from a vendor
project. Output parents must exist; aliased destinations and input replacement
are rejected. The source-only context never invents authenticated artifact hashes:
artifact-scoped facts require a different, explicitly authenticated consumer.

[Contracts](../contracts/README.md) owns semantic identities, evidence classification
and applicability without execution dependencies. [Review](../review/README.md) owns
reviewed-pack validation and conflict selection. [Model](../model/README.md) owns
geometry, hardware semantics and PAC policies. The tool loads one model and policy
set, validates their evidence/ownership and renders all outputs from those values.
Reading several source documents is not an atomic filesystem snapshot; the loaded
values, rather than reopened paths, supply generation.

The host owns temporary files and the `rustfmt` process group. Formatting has a
60-second timeout and cancellation reaps the child group. Generated Rust is limited
to 128 MiB before reading it back. These trusted source-generation inputs and
third-party SVD rendering do not claim allocation-controlled binary analysis.
Binary decoding and dataflow are owned by the analysis tools. The signature
producer below consumes their public output.

Publication checks reference validity, selected applicability, reviewed semantics,
memory ranges, API safety and generated source consistency. They do not qualify
hardware or prove equivalence to vendor behavior. Review policy, source provenance
and required recovered tables remain with `registers/`.

## Register checks

Native analog-I2C model documents are validated with the same publication
and written to the explicit `[outputs].analog` path as the PAC's `analog_i2c`
data/value module. The PAC host tests compile it and check its routes against
the bus transport. Production bus callers use the policy field domain.
CPU SVD and CPU bindings omit indirect spaces.
`cargo registers inspect-analog --manifest <publication>`
prints their complete native inventory; see the [model format](../model/README.md#analog-i2c-space).

`oer_registers_generate::checks` holds the register rules, so no caller keeps its
own copy:

- `pac_transactions::check`: a handwritten PAC module
  (`chips/*/pac/src`) never loops, except a `while` walking an
  index over a fixed-length table; polling and retries belong to the HAL.
  `cargo registers check pac-transactions <directory>` runs it.
- `shared_words::check`: every MMIO word a write transaction of the radio
  PAC (`policy/api.toml`, through the published SVD) or a typed block's
  claim reaches (every declared word of a typed peripheral) and pinned
  esp-hal both store to has a reviewed entry in `chips/<chip>/registers/shared-words.toml`, and
  no reviewed word is no longer shared. esp-hal's stores include the clock
  and reset writers of the chip's `esp-metadata-generated` file, and its
  peripheral names resolve through that file's singleton aliases
  (`SYSTEM <= PCR`). `cargo registers check shared-words --chip <chip>
  --esp-hal-src <dir> --esp-metadata-generated-src <dir> --pac-src <dir>`
  runs it on the given esp-hal, esp-metadata-generated and PAC sources.
  esp-hal stores through `P::regs()` and through `let p = unsafe {
  P::steal() };` bindings count; an array accessor spans its `[(); N]`
  elements at its `.add(S * n)` stride, and a byte register counts as the
  word that holds it. An entry with `words = N` reviews a run of `N` words
  four bytes apart from its `address`. The vendor SVD's `soc` blocks are
  not in the check: no code claims them while esp-hal owns the platform.

`ChipSources` resolves the files a chip's publication manifest names
without validating them. The binding index format is
`oer_registers_bindings::BindingIndex` ([`bindings/`](../bindings/README.md)), a
foundation data format the generator writes and every reader parses.

A chip publication's `[outputs]` name the SVD and the binding index; its
optional `[outputs.pac]` (`raw`, `api`, `crate-name`, `target`, `edition`)
adds the raw PAC and its API facade, and a publication has a PAC API policy
(`api`) exactly when it has that table. A publication without one, a chip's
`platform.toml`, describes registers the host reads through JTAG and that
no firmware reaches: its binding index names no crate.

A chip publication with a PAC may name `[soc]`: `svd` (the vendor SVD,
unmodified), `overlay` (the chip's strictness overlay) and `report` (the
generated report). The vendor SVD renders into the PAC API's `soc` module;
see the [register model library](../model/README.md#the-vendor-svd). The
overlay's ESP-IDF headers are `esp-idf/<path>` artifacts of the chip's
`vendor/artifacts.toml`, read verified from the vendor store: generation
fails, naming `cargo pins fetch --chip <chip>`, while one is missing, and
when the overlay's `esp-idf` pin is not the manifest's.

## Strictness overlay from ESP-IDF headers

`cargo registers overlay check --chip <esp32s31|esp32c5> --idf <checkout>`
verifies every entry of the chip's `model/strictness.toml` against the file
and line it cites in the pinned ESP-IDF register headers and against the
vendor SVD: the register definition line (`*_reg.h`), the field line (name,
bits, access class), the closing line of the register union (`*_struct.h`,
whose `reserved_*` members are the entry's `reserved` bits and whose other
members are the SVD's fields) and the enumerator of `interrupts.h` (its
position is the source number). An access class decides what an entry may
state: `R/W` a plain field, `WT` a write trigger, `HRO` a hardware-modified
read-only field, a hardware-modified field (`SC`, `SS`, ...) a read-only
register, all fields `WTC` a write-one-to-clear register. `trm:` and
`esp-hal:` sources, `read-side-effect` and enumerated values are counted as
unchecked.
The checkout must be at the overlay's `esp-idf` pin.

`cargo registers overlay import --chip <chip> --idf <checkout>
[--peripheral <name>]...` rebuilds the entries from the headers and writes
the overlay: for the registers it already states, or for all registers of the
named peripherals. It keeps `fifo` and `read-side-effect`, which no header
states, and writes the interrupt sources the SVD lacks.

## Pinned header function signatures

`cargo registers signatures --chip <esp32s31|esp32c5> --idf <checkout>
--sdkconfig <build/config/sdkconfig.h> [--libc-include <picolibc/include>]
[--clang clang] [--check] [--output <ignored-report-directory>]` writes
`chips/<chip>/vendor/facts/idf-function-signatures.toml` and its companion
`idf-function-signatures.provenance.toml`. Use the complete ESP-IDF checkout
at the chip's current pin, with its pinned header submodules initialized.
`--check` repeats generation and requires byte-for-byte equal facts.

The SDK configuration is an explicit input. For the checked-in facts, use
the generated configuration from `cargo hil firmware build
ble-scan-vendor-reference` (S31) or `cargo hil firmware build
ble-scan-request-counter` (C5). These build SDK reference firmware locally.
The configuration hash is recorded, and another configuration may expose a
different set of declarations. The command neither flashes nor captures a
board. A RISC-V capable Clang and the SDK's C library headers are required.

The ELF facade supplies global name candidates from every pinned binary;
Blobray `call-arguments --call-summaries --abi riscv-integer` selects actual
named call targets. This includes externally linked functions implemented
elsewhere in the corpus. Coverage reports separately identify targets with
no captured executable code bytes; an absolute import symbol alone does
not establish a captured implementation. Partial analysis is reported and
does not prove absence of further calls.

The generator compiles matching open headers as RV32 ILP32 C, follows the
compiler's resolved type graph and queries sizes in that same translation
unit. It publishes four fields per `[[function]]`: `name`, `arguments`,
`result`, `source`. `arguments` counts occupied a0..a7 words, capped at eight:
`(void *, uint64_t)` consumes three. Named 64-bit scalars occupy consecutive
words under the [integer psABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention).
Stack arguments do not allow discarding a register. Results are `void`,
`u32` (pointers or integers of at most 32 bits), or `u64` (64-bit integers).
Aggregates by value, variadic functions, floating types, old-style
declarations and other unsupported types supply no contract. A compiler
error invalidates the whole header context. Conflicting supported contracts
or supported/unsupported declaration variants stop publication.

Each record cites a pinned header path and line, followed by a header hash
comment. The companion provenance records header hashes, SDK configuration
hash and complete binary input hashes without local cache paths. Published
headers must match committed bytes, including the parent commit's gitlink
for a header in a submodule. Facts are Apache-2.0; the generator remains
under the workspace license. It copies no source declarations or code.

The ignored report directory defaults to `target/register-signatures/<chip>`.
The child Blobray uses `--analysis-profile signature-corpus` (256 MiB,
1,000,000,000 work units, 3600 seconds). The schema-1 `coverage.json` retains
its complete `corpus.analysis_resources` without recalculating the child counts.
The directory contains the raw call capture, compiler diagnostics and `coverage.json`:
covered names, targets without captured definitions, unsupported prototype
reasons, failed header contexts, identifiers without qualified declarations
and names absent from all selected headers. Identifier search selects work;
only compiler-qualified declarations create facts. Header coverage does not
establish agreement with binary call behavior: integration with the signature
consumer must report any contract/summary conflict before using a contract.
