# Chip facts

`oer-espressif-facts` resolves the facts of an Espressif chip from TOML
layers into one checked model, [`Facts`](src/lib.rs). It touches no hardware
and no file system (a `Source` reads the files) and builds for the chip
targets; `oer-chip-profile` adds the image registry and is what the host
tools load.

## Layers

| Layer | File | Holds |
| --- | --- | --- |
| ISA | `chips/espressif/platform/isa/<isa>.toml` | the ISA of the harts and its bare-metal Rust target |
| family | `chips/espressif/platform/family.toml` | the boot flow, the interrupt-table layout and symbols, the placement contract, the staged boot's host contract, the reachability report's observed functions |
| chip | `chips/<id>/platform/chip.toml` | the chip's properties, flash map, address map, harts, core start, ROM, reset cause; still the image registry (`[hil]`, `[packages]`, `[[probe]]`) |
| module | `chips/<id>/platform/modules/<module>.toml` | the module that carries the chip: its flash and PSRAM |
| board | `chips/<id>/platform/boards/<board>.toml` | the board that carries the module |

A chip resolves from its board (at most one per chip until images name
theirs), down through its module and chip layer to the ISA and the family.

## Rules

- Every layer file has `schema = 2`. `extends` names its parents, one path
  or a list, relative to the file; the chip layer extends its ISA and the
  family, a module its chip, a board its module.
- Parents share no key, and no layer is reached twice or in a cycle.
- A layer adds keys to its parents. A key a parent already sets is an
  override, and the layer names each one with its source:
  `[override] "dotted.key" = "SOURCE: ..."`. An override of a key no parent
  sets is refused.
- Tables merge key by key; any other value, an array included, is one fact.
- A required key missing after resolution and a key the model does not know
  are refused by name.

## Derived facts

- `[flash] application-encoding.size-mib` is the module's flash size.
- `[memory] psram` is the module's PSRAM at the chip's `psram-origin`;
  `runtime-psram` is the PSRAM after the family's
  `[staged] bootstrap-psram-bytes`.
- A family `[placement]` rule with `per-hart = true` stands once for each
  hart of the chip's `[interrupts] harts`, `{hart}` replaced by its number;
  `bytes-per-hart` gives a rule `bytes` of that many per hart.
- `rust-target` is checked against `isa`; `[core-start]` is named by exactly
  the chips with more than one core.

`tracked` holds the tracked layer files as the build compiled them in, for
host tests and for tools that detect a tree newer than themselves; a test
holds the list to the files in the tree.
