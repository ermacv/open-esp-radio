# Reviewed register publication

`oer-registers-generate` validates source-owned hardware descriptions and generates
SVD, raw PAC, restricted PAC API and the binding index. It does not load a binary
analysis project, vendor binary, device model or global provider registry.

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
`library` instead of a `model` publishes that library: svd2rust renders its
layouts at placeholder bases, the tool keeps only the register modules, makes
each peripheral alias the bare `RegisterBlock` and appends the library API's
transactions. It writes the raw crate and API bridges, but no SVD or binding
index, since a library has no addresses. A chip that places a library layout
re-exports the library's module from its raw PAC and may not define
transactions on that peripheral. See [`chips/espressif/registers/ieee80211`](../../../chips/espressif/registers/ieee80211/README.md).

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
with `element = "<LAYOUT>"` and `count`. The element must be a shared word
layout, with the same alignment and placement as the array, whose size is
the element size times count. The generated array exposes checked
`entry(index)` access and is itself a typed pointer target.

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
word layout a `#[repr(C, align(N))]` type of volatile cells with `zeroed`,
`SIZE`, `ALIGN`, `PLACEMENT` and `bytemuck::Zeroable`, each cell with `read`
and `write` of a value type, which has `from_bits`, `bits`, a getter per
field and a `with_*` builder (`Option<Self>` when the value may not fit);
per byte layout a decoder over `&[u8; N]` with a value type per range and a
getter per field; a byte layout with CPU-written fields also gains a writer
over `&mut [u8; N]` (`set_<range>` and the range getter) and `from_bits` and
`with_*` builders on the ranges the CPU writes, never on a hardware-written
field. When an address field exists it also gains the typed
addresses of the DMA region: `DmaWindowAddress` for a buffer and
`DmaRef<L>` for a layout placed there (the sealed `DmaLayout`), both with an
`unsafe` constructor only `oer_esp32s31_hal::dma_address` calls. An address
field's builder takes that type (`Option<DmaRef<L>>` for a link, `None` for
none); its getter returns the word as read. No ordering, fence or ownership
is generated.

A chip publication may name `[outputs.linker]`: `path` and
`address-low-field = "PERIPHERAL.REGISTER.FIELD"`. It writes a linker
fragment with `DMA_WINDOW_ORIGIN` and `DMA_WINDOW_LENGTH` (the memory map's
`dma-window` region) and `DMA_ADDRESS_LOW_BITS` (that field's width, from
bit 0), which the runtime linker script includes.

A complete-register write in the API policy takes either a `domain` or, for
a register the hardware reads as a pointer, `points-to = "<LAYOUT>"`: its
bridge then takes a `DmaRef` to that layout of the model.

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
Binary investigations are outside this tool.

Publication checks reference validity, selected applicability, reviewed semantics,
memory ranges, API safety and generated source consistency. They do not qualify
hardware or prove equivalence to vendor behavior. Review policy, source provenance
and required recovered tables remain with `registers/`.

## Register checks

`oer_registers_generate::checks` holds the register rules, so no caller keeps its
own copy:

- `pac_transactions::check`: a handwritten PAC module
  (`chips/*/pac/src`) never loops, except a `while` walking an
  index over a fixed-length table; polling and retries belong to the HAL.
  `cargo registers check pac-transactions <directory>` runs it.
- `shared_words::check`: every MMIO word a write transaction of the radio
  PAC (`policy/api.toml`, through the published SVD) and pinned esp-hal both
  store to has a reviewed entry in `chips/<chip>/registers/shared-words.toml`, and
  no reviewed word is no longer shared. `cargo registers check shared-words
  --chip <chip> --esp-hal-src <dir> --pac-src <dir>` runs it on the given
  esp-hal and PAC sources.

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
