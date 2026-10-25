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
| family | `chips/espressif/platform/family.toml` | the boot flow, the CLIC's lines and levels, the interrupt-table layout and symbols, the placement contract, the staged boot's host contract (MMU page, image headers, probe page, handoff margin), the reachability report's observed functions |
| chip | `chips/<id>/platform/chip.toml` | the chip's properties, flash map, address map (internal SRAM, flash window, retained memory) with each region's attributes, what the boot chain keeps (`[boot-chain]`), harts, core start, reset cause; still the image registry (`[hil]`, `[packages]`, `[[probe]]`) |
| ROM | `chips/<id>/platform/rom-<revision>.toml` | the mask ROM of one chip revision: its pinned ELF, symbol script, sourced entry points (`[[rom.functions]]`), function summaries, forbidden code and what it uses of the internal SRAM (`[rom] sram`) |
| module | `chips/<id>/platform/modules/<module>.toml` | the module that carries the chip: its flash and PSRAM |
| board | `chips/<id>/platform/boards/<board>.toml` | the board that carries the module |

A chip resolves from its board (at most one per chip until images name
theirs), down through its module, the ROM of the module's chip revision and
the chip layer to the ISA and the family.

## Rules

- Every layer file has `schema = 2`. `extends` names its parents, one path
  or a list, relative to the file; the chip layer extends its ISA and the
  family, a ROM layer its chip, a module the ROM of its chip's revision, a
  board its module.
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
- `[memory] sram`, the application's SRAM, is the chip's `internal-sram`
  below the boot chain's reserve, `[boot-chain] application-sram-end` (the
  whole internal SRAM on a chip without a boot chain).
- `[memory] psram` is the module's PSRAM at the chip's `psram-origin`;
  `runtime-psram` is the PSRAM after the family's
  `[staged] bootstrap-psram-bytes`.
- A family `[placement]` rule with `per-hart = true` stands once for each
  hart of the chip's `[interrupts] harts`, `{hart}` replaced by its number;
  `bytes-per-hart` gives a rule `bytes` of that many per hart.
- A rule with `per-line = true` numbers `{n}` by every vector-entry line, 1
  to `[interrupts] lines` less one.
- `rust-target` is checked against `isa`; `[core-start]` is named by exactly
  the chips with more than one core; SRAM lies below PSRAM, flash and PSRAM
  in the shared `cache-window` of a chip that has one, and the boot chain's
  loader and retained tail in SRAM and the retained memory.
- `[memory.attributes]` names exactly the regions the chip maps (`sram`,
  `flash-xip`, and `psram` and `retained` where mapped), each `executable`,
  `cached`, `dma` (in the radio's DMA window), `retained` (no reset entry
  initializes it) and `writable`; `runtime-psram` has the PSRAM's. The host
  checks classify memory by these, never by a region's name.
- The ROM's `[rom] sram` ranges lie in the internal SRAM in address order,
  each `kept` during `download` mode only, until the `boot` chain hands over,
  or while an image is `running`; a `running` range (the ROM's `.data` and
  `.bss`) inside the application's SRAM is refused. `[rom] revision` is one
  of the chip's `revisions`.

## Generated

`generate` turns the facts into the sources a person writing them could get
wrong, each a pure function of the facts; build scripts call it, and nothing
it writes is in git:

- `tracked_layout`: a chip's `LAYOUT` (`oer-espressif-staged-layout`), from
  its facts and its stack policy's `default_cpu0_task_stack_bytes`
  (`chips/<id>/platform/stack.toml`), which its layout crate's build writes
  and every `--defsym` of the linker scripts comes from;
- `Vectors`: the staged runtime's vector tables, every hart's line entries
  (`RUNTIME_IRQ_ENTRIES`, `RUNTIME_VECTOR_TABLES` around the hand-written
  entry and exit of `vectors.s`), every line's default handler (the linker
  fragment `oer-staged-runtime-vectors.x`) and the interrupt table's
  `LINES`, from the chip's harts and CLIC lines.
- `rom_script`: the ROM linker assignments from `[[rom.functions]]`, each
  with a `name`, `address` and pinned `SOURCE:` citation in `source`. The
  S31 layout build writes `[rom] linker-script` under its `OUT_DIR` and
  stages the register-model-generated DMA window beside it for the shared
  linker binding. A host test scans our S31, shared chip and firmware
  foreign function declarations and requires exactly the same export set;
  the provenance test restricts S31 exports to the pinned nonradio script.
  Radio ROM and vendor libc/compiler-intrinsic script provenance and
  forbidden ROM addresses are refused before generation. The pinned
  compiler_builtins enables its `mem` routines for bare-metal `-none`
  targets, including S31's `riscv32imafc-unknown-none-elf`.

`tracked` holds the tracked layer files and each chip's stack policy as the
build compiled them in, for host tests, the generating build scripts and
tools that detect a tree newer than themselves; a test holds the list to the
files in the tree.
