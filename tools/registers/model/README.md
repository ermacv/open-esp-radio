# Register model library

## Block types and the chip root

`chip_model::ChipModel::load(root, evidence)` reads a chip as one root file
and the block type files it includes. Generation still reads the
schema-3 manifest and its schema-2 fragments described below; that format
is deleted in the same change that moves the generator to this one, and no
adapter joins the two.

A **block type** is one hardware block, one TOML file, without an address:
every register has an offset from the block's base. A type is never a
slice of a block by owner or function; who owns which words is policy.
Blocks with a different register set are different types. The **root**
includes the types, places their **instances** at absolute addresses, and
holds the chip's interrupt sources, the analog-I2C space, the RAM regions
and the placement of memory layouts.

### Block type file (`schema = 3`)

```toml
schema = 3

[type]
name = "TIMG"
description = "Timer group: two timers and a watchdog."
space = "mmio"                    # or "analog-i2c": byte registers by index
sources = ["IDF_TIMG_REG_H"]

[[interrupt-outputs]]             # named outputs; instances map them to sources
name = "T0"
sources = ["IDF_TIMG_REG_H"]

[review]                          # every register and field, unless overridden
sources = ["IDF_TIMG_REG_H"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[review.opaque]                   # registers and fields named `*_OPAQUE`
sources = ["BLOB_TIMG_USE"]       # (naming = "opaque" is implied)
provenance = "observed"
accuracy = "positional"
completeness = "unknown"

[review.entity."INT_RAW.T0"]      # only the keys that differ from the default
accuracy = "bounded"

[[registers]]
name = "INT_RAW"
offset = 0x70                     # from the block's base
width = 32                        # 8, 16 or 32 (mmio, default 32); 8 (analog-i2c)
description = "Raw interrupt status."
reset = { value = 0, mask = 0xffffffff }   # mask: the bits whose reset is known
reserved = { mask = 0xfffffff8, sources = ["IDF_TIMG_REG_H"] }

[[registers.fields]]
name = "T0"
bits = "0"                        # "n" or "high:low"
class = "write-one-to-clear"

[[registers]]
name = "WORD_0080_OPAQUE"         # positional name: offset from the block's base
offset = 0x80

[[registers.fields]]
name = "BITS_0_7_OPAQUE"
bits = "7:0"
class = "plain"

[[registers]]
name = "REGCLK_%s"                # one repeated word: `dim`, `%s` is the index
offset = 0x90
dim = { count = 2, stride = 4 }

[[registers.fields]]
name = "DIV"
bits = "7:0"
class = "plain"
write-range = { min = 1, max = 255 }
values = [{ name = "BYPASS", value = 1, description = "Undivided." }]

[[groups]]                        # a repeated group of registers
name = "T%s"                      # expanded `T0`, `T1`
offset = 0x00
count = 2
stride = 0x24

[[groups.registers]]
name = "CONFIG"
offset = 0x00                     # from the start of the group

[[groups.registers.fields]]
name = "EN"
bits = "31"
class = "plain"

[[groups.registers]]
name = "LOAD"
offset = 0x20

[[groups.registers.fields]]
name = "LOAD"
bits = "0"
class = "write-trigger"

[[registers]]
name = "CMD"
offset = 0xa0

[[registers.fields]]
name = "READ"
bits = "0"
class = "self-clearing"

[[registers.fields]]
name = "ERASE"
bits = "1"
class = "self-clearing"

[[registers.fields]]
name = "BUSY"
bits = "2"
class = "self-clearing"

[[registers.fields]]
name = "WHOLE_CHIP"
bits = "3"
class = "self-clearing"

[[commands]]                      # a command register
register = "CMD"                  # `GROUP.REGISTER` inside a group
commands = ["READ", "ERASE"]
qualifiers = ["WHOLE_CHIP"]
held = ["BUSY"]
sources = ["IDF_TIMG_REG_H"]
```

Every field has one `class` from a closed set. The class alone fixes what a
read and a write do; there is no separate access key.

| `class` | Header classes | What the renderer can generate |
| --- | --- | --- |
| `plain` | `R/W` | getter, setter and read-modify-write |
| `read-only` | `RO`, `R`, `HRO`, `RO/SS` | getter only |
| `write-only` | `WO` | setter in a write from zero; no getter |
| `write-trigger` | `WT` | `trigger_<register>_<field>` (a wider one with its value); zero in every other write; no getter |
| `write-one-to-clear` | `WTC`, `W1C` | in a register of only such fields: a sample token and its acknowledgement; otherwise getter only |
| `set-only` | `W1S`, `WTS`, `WS`, `R/W1` | getter only |
| `self-clearing` | `R/W/SC`, `R/W/WTC` | getter only, unless a `[[commands]]` entry names it |
| `hardware-written` | `R/W/SS`, `R/W/SS/SC`, ... | getter only, unless it is a data port's field |
| `read-clears` | `R_SS_RC`, "cleared by a read" | nothing: its register has no API |
| `debug-only` | "Do not use it in applications" | getter only; never written |
| `unknown` | none: no source states the access | nothing: neither a read nor a write |

Register-level constructs live in the type file:

- `[[commands]]`: `commands` are one-bit `self-clearing` fields software
  starts; `qualifiers` are one-bit `self-clearing` fields a start writes by
  its argument; `held` names every other one-bit `self-clearing` field of
  the register. Together they are the register's exclusive group: a start
  refuses, writing nothing, while any of them reads one. A command register
  has no `write-trigger`, `read-clears` or `unknown` field and no port or
  read side effect;
- `fifo = { sources }`: a FIFO port, popped and pushed only; one field, no
  `dim`, neither `write-one-to-clear` nor `unknown`;
- `data-port = { sources }`: a command port's data word, read and stored
  whole; one `plain` or `hardware-written` field;
- `read-side-effect = { sources }`: a read changes the hardware; no API;
- `reset = { value, mask }` and `reserved = { mask, sources }`: reserved
  bits are never fields and every write keeps them;
- analog-I2C types only: `read-safe = { epoch, comparison-mask, sources }`
  and `[[registers.views]]` (`name`, `field`, `bits`, `sources`, a strict
  subrange of one field). Read-safe qualification needs readable fields,
  and its comparison mask covers only readable field bits; without a mask
  it qualifies all declared fields. `write-only` and `write-trigger` have
  no readable value, and neither has `unknown`. FIFO, read-side-effect and
  read-clears registers never qualify, even when the comparison mask
  excludes the affected bits.

A whole-word fixed image is a transaction, so it stays in the PAC policy
(`fixed-register-images`, `whole-word = true`).

`dim` repeats a single word (`count` at least 2, `stride` a multiple of
the register's bytes, the name holds one `%s`). `[[groups]]` repeat a set
of registers, which take their offsets from the group's start; the group
name holds one `%s`, `count` is at least 2, `stride` covers every register
of the group, and a group holds no group. The group's offset and stride
preserve every member's natural alignment, including members with `dim`.
No two words of a type overlap:
registers, `dim` elements and group elements alike, and no register or
other group lies inside a group's span, from its first element's start to
its last element's end. An analog byte's index is below 256. Repeated spans
are checked before expansion. The loader bounds materialization for all
included types combined, and separately for the chip's expanded instances
and root relations, to 65,536 records and 64 MiB of owned expansion
text (including source-string slots); excessive expansion returns a typed
error before creating those records.

Review is by exception. `[review]` is the default record of every register
and field, `[review.opaque]` of every one whose name ends in `_OPAQUE`
(with `naming = "opaque"`). A `[review.entity."KEY"]` with `KEY` one of
`REGISTER`, `REGISTER.FIELD`, `GROUP.REGISTER` or `GROUP.REGISTER.FIELD`
(the template names, `%s` included) overrides the keys it states, and only
keys that differ. Every explicitly supplied source list is validated even
when overridden or unused. After the merge every register and field has
sources, non-hint provenance, accuracy, completeness and naming; a hint
cannot supply reviewed coverage. The naming rules of
[Name origin](#name-origin) apply, with a positional name's offset taken
from the block's base (a group register at the group's first element; a
`dim` register inserts `_%s`), and `BYTE_<index>_OPAQUE` for an analog
register. The first group member's combined offset must fit `u32`, the
positional naming domain. An observation of one instance is a source of
that instance in the root, not a second review.

An entity no source is recorded for has an override with
`unsourced = "<reason>"` and at most a `naming` that differs from its
default. It states no `sources`, `provenance`, `accuracy` or
`completeness` and inherits none: it resolves to no sources, no
provenance and `unknown` accuracy and completeness, so it claims nothing
beyond its bits and name and is no reviewed assertion. A `vendor` name
needs a source, so an unsourced entity is `descriptive` or `opaque`.
`ChipModel::unsourced_reviews` lists every such review with its reason.

```toml
[review.entity."CONTROL.GAIN"]
unsourced = "The curated review names it without a source."
```

### Chip root file (`schema = 4`)

```toml
schema = 4
chip = "esp32s31"
interrupt-sources = { count = 169, sources = ["IDF_INTERRUPTS_H"] }
types = ["types/timg.toml", "types/bbpll.toml"]   # relative to the root
layouts = ["layouts/rx-descriptor.toml"]

[[instances]]
name = "TIMG0"
type = "TIMG"
base = 0x20580000
extent = { size = 0x1000 }        # covered by the instance's sources
sources = ["IDF_REG_BASE_H"]

[[instances]]
name = "BT_CORE"
type = "BT_CORE"
base = 0x20101000
extent = { size = 0x800, partial = true, sources = ["BLOB_BT_WORDS"] }
sources = ["BLOB_BT_WORDS"]

[[interrupts]]
name = "TG0_T0"
source = 25
instance = "TIMG0"
output = "T0"                     # required when the type names outputs
sources = ["IDF_INTERRUPTS_H"]

[[analog-i2c.instances]]
name = "BBPLL"
type = "BBPLL"                    # a type with space = "analog-i2c"
code = 0x66
sources = ["IDF_REGI2C_BBPLL_H"]

[[analog-i2c.instances.routes]]
profile = "phy"
request-code = 0x66
host = 0
read-select = 0
read-mask-bits = 32
sources = ["IDF_REGI2C_CTRL"]

[[ram-regions]]
name = "dma-window"
base = 0x2f000000
size = 0x80000
sources = ["IDF_SOC_H_DMA"]

[[layout-placements]]
layout = "RX_DESCRIPTOR"
regions = ["dma-window"]
```

An instance has no per-instance patch: TIMG0 and TIMG1 are two instances of
one type. Every word of its type lies inside its extent, and no two MMIO
instances or RAM regions overlap. A `partial` extent is the part of a block
the sources show, with its own sources; a block known only from vendor
binaries is a descriptive type with such an extent. `interrupt-sources`
is required: its sourced `count` declares the complete source range
`0..count` (at most 65536 sources). Each number occurs exactly once in
`[[interrupts]]` or `[[unbound-interrupts]]`; names are unique across both
lists. A bound source names an existing instance and, when its type names
outputs, one of them; several sources may lead to one output. An unbound
entry has `source`, `name`, a nonblank `reason`, and `sources`. Missing,
out-of-range, repeated or simultaneously bound/unbound sources fail loading.
The root example is an excerpt: the other S31 sources must also be listed.
Analog-I2C instances are addressed by their block code, not by a CPU
address, and keep their request routes. Memory layout files keep their
own format without a file-level `placement`; the root may place a layout
in RAM regions, which an address field's `placement` also names. Omitting
`[[layout-placements]]` for an included layout leaves it unplaced: it can
still describe bytes or words, but is not a DMA target. A `points-to`
target must be a word layout or word array placed in `dma-window` only.
After root placement, the loader checks pointer targets, equal array and
element placements, and `same-geometry-as` (including equal placement).

Loading expands every instance: each register element and field gets an
absolute address (an `AnalogAddress` in the analog space) and the identity
`INSTANCE.REGISTER.FIELD`, with `%s` replaced by the `dim` or group index
(`TIMG0.T1.CONFIG.EN`). Every `sources` entry must exist in the evidence
catalogs. Every check here runs on load and fails with a typed
`chip_model::Error` that names the file and the entity.

### Fixed clocks, resets and selectors

An MMIO instance's `clocks` and `resets` are lists of `{ role, field,
sources }`; `muxes` adds `inputs = { encoding = "CLOCK_SOURCE" }`.
`field` is an absolute expanded `INSTANCE.REGISTER.FIELD` identity,
including concrete `dim` and group indices. The referenced instance must
be a different MMIO instance of the root; links to the instance's own block
are rejected. Gates and resets require one `plain` bit; a mux requires a
`plain` field. A field has one control kind across all instance links and
gate groups: clock gate, reset or mux. Roles are nonempty segments of ASCII
lowercase letters and digits separated by single hyphens, unique within each
list; leading, trailing and consecutive hyphens are rejected. Several roles
of one instance still count as one user. Input keys are canonical unsigned
decimal numbers within the field's width; every value names a declared
`[[clock-sources]]` entry.
The input map is nonempty and identical for all users of one selector;
unlisted encodings remain unknown.

Every field used by at least two distinct instances requires one named
`[[shared-controls]]` entry with `field`, `instances` and `sources`.
The name and field are unique; `instances` lists exactly its users, each
once, at least two. Each user keeps its own role and sourced field link.
Grouping fields in one vendor word does not make them shared controls:
sharing concerns the same field, not adjacent bits or policy dependencies.

`[[gate-groups]]` names a vendor's fixed group of distinct one-bit `plain`
fields, with `fields` and `sources`. It has no consumers or ordering;
group membership does not count as another instance using a field. A group
may include a linked clock gate or a gate without instance links, but never
a field linked as a reset or mux.
`[[clock-sources]]` has a unique `name`, `sources`, optional positive
`frequency-hz`, and optional `parent` naming another clock source. Parent
cycles and unknown parents are errors; unknown frequencies stay absent.
Names of shared controls, gate groups and clock sources are `UPPER_SNAKE`.
All new source lists obey the same evidence-catalog checks as type sources.
Controls, mux inputs, shared-control users, gate-group fields and clock
sources share the chip expansion budget. Copied identities, roles, input
names and source-string slots count toward its text limit. Budget checks
precede relation expansion; parent-chain validation visits each clock once.

There are no power-domain or instance `parents` fields: S31 subsystem
membership and reset propagation are not established by the sources.
Known common controls use explicit field links and shared controls.
Reference counts, module dependency sets, enable/reset order, writers,
retention and fixed write sequences belong to HAL or PAC policy. In-block
register-clock fields and interrupt status/acknowledgement banks belong to
the block type.

These are S31 root excerpts. Included types, other sources and modem
instance placements are omitted. Evidence IDs below denote entries for
ESP-IDF `4d59230ddff16327812782151ef0afef202dc6d7`:

| Evidence ID | Source under `components/` |
| --- | --- |
| `IDF_REG_BASE_H` | `soc/esp32s31/register/soc/reg_base.h:8-144` |
| `IDF_TIMG_LL` | `esp_hal_timg/esp32s31/include/hal/timg_ll.h:35-68` |
| `IDF_TIMER_LL` | `esp_hal_timg/esp32s31/include/hal/timer_ll.h:100-158` |
| `IDF_MWDT_LL` | `esp_hal_timg/esp32s31/include/hal/mwdt_ll.h:273-297` |
| `IDF_USB_LL` | `esp_hal_usb/esp32s31/include/hal/usb_serial_jtag_ll.h:337-350` |
| `IDF_MODEM_IMPL` | `esp_hw_support/modem/port/esp32s31/modem_clock_impl.c:17,19,178-181` |
| `IDF_MODEM_SYSCON_LL` | `hal/esp32s31/include/hal/modem_syscon_ll.h:401-412` |
| `IDF_MODEM_CLOCK_HAL` | `hal/esp32s31/modem_clock_hal.c:18-20,203-212,242-251,281-290` |
| `IDF_INTERRUPTS_H` | `soc/esp32s31/include/soc/interrupts.h:19-188` |

```toml
[[instances]]
name = "TIMG0"
type = "TIMG"
base = 0x20580000
extent = { size = 0x1000 }
sources = ["IDF_REG_BASE_H"]
clocks = [
  { role = "bus", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_APB_CLK_EN", sources = ["IDF_TIMG_LL"] },
  { role = "t0", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_T0_CLK_EN", sources = ["IDF_TIMER_LL"] },
  { role = "t1", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_T1_CLK_EN", sources = ["IDF_TIMER_LL"] },
  { role = "wdt", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_WDT_CLK_EN", sources = ["IDF_MWDT_LL"] },
]
resets = [
  { role = "group", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_RST_EN", sources = ["IDF_TIMG_LL"] },
]
muxes = [
  { role = "t0", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_T0_SRC_SEL", inputs = { 0 = "XTAL", 1 = "RC_FAST", 2 = "REF_F80M" }, sources = ["IDF_TIMER_LL"] },
  { role = "t1", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_T1_SRC_SEL", inputs = { 0 = "XTAL", 1 = "RC_FAST", 2 = "REF_F80M" }, sources = ["IDF_TIMER_LL"] },
  { role = "wdt", field = "HP_SYS_CLKRST.TIMERGRP0_CTRL0.REG_TIMERGRP0_WDT_SRC_SEL", inputs = { 0 = "XTAL" }, sources = ["IDF_MWDT_LL"] },
]

[[instances]]
name = "USB_SERIAL_JTAG"
type = "USB_SERIAL_JTAG"
base = 0x20391000
extent = { size = 0x1000 }
sources = ["IDF_REG_BASE_H"]
clocks = [
  { role = "bus", field = "HP_SYS_CLKRST.USB_DEVICE_CTRL0.REG_USB_DEVICE_APB_CLK_EN", sources = ["IDF_USB_LL"] },
  { role = "48m", field = "CNNT_SYS.SYS_HP_USB_DEVICE_CTRL.SYS_USB_DEVICE_48M_CLK_EN", sources = ["IDF_USB_LL"] },
]
resets = [
  { role = "block", field = "CNNT_SYS.SYS_HP_USB_DEVICE_CTRL.SYS_USB_DEVICE_RST_EN", sources = ["IDF_USB_LL"] },
]

# Excerpts of modem instances: their sourced placements are separate facts.
[[instances]]
name = "BT_BB"
type = "BT_BB"
clocks = [{ role = "baseband", field = "MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN", sources = ["IDF_MODEM_IMPL"] }]

[[instances]]
name = "IEEE802154"
type = "IEEE802154"
clocks = [{ role = "baseband", field = "MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN", sources = ["IDF_MODEM_IMPL"] }]

[[shared-controls]]
name = "BT_I154_COMMON_BB"
field = "MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN"
instances = ["BT_BB", "IEEE802154"]
sources = ["IDF_MODEM_IMPL"]

[[instances]]
name = "MODEM_PWR"
type = "MODEM_PWR"
muxes = [{ role = "32k", field = "MODEM_LPCON.MODEM_32K_CLK_CONF.CLK_MODEM_32K_SEL", inputs = { 0 = "XTAL32K", 1 = "RC32K", 2 = "EXT32K" }, sources = ["IDF_MODEM_CLOCK_HAL"] }]

[[instances]]
name = "COEX"
type = "COEX"
muxes = [{ role = "32k", field = "MODEM_LPCON.MODEM_32K_CLK_CONF.CLK_MODEM_32K_SEL", inputs = { 0 = "XTAL32K", 1 = "RC32K", 2 = "EXT32K" }, sources = ["IDF_MODEM_CLOCK_HAL"] }]

[[shared-controls]]
name = "MODEM_32K"
field = "MODEM_LPCON.MODEM_32K_CLK_CONF.CLK_MODEM_32K_SEL"
instances = ["MODEM_PWR", "COEX"]  # MODEM_PWR has LP-timer and Wi-Fi-power outputs
sources = ["IDF_MODEM_CLOCK_HAL"]

[[gate-groups]]
name = "WIFI_BB"
fields = [
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_22M_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_40M_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_80M_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_40X_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_80X_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_40X1_EN",
  "MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_160X1_EN",
]
sources = ["IDF_MODEM_SYSCON_LL"]

[[clock-sources]]
name = "XTAL"
sources = ["IDF_TIMER_LL"]
[[clock-sources]]
name = "RC_FAST"
sources = ["IDF_TIMER_LL"]
[[clock-sources]]
name = "REF_F80M"
sources = ["IDF_TIMER_LL"]
[[clock-sources]]
name = "XTAL32K"
sources = ["IDF_MODEM_CLOCK_HAL"]
[[clock-sources]]
name = "RC32K"
sources = ["IDF_MODEM_CLOCK_HAL"]
[[clock-sources]]
name = "EXT32K"
sources = ["IDF_MODEM_CLOCK_HAL"]

[[interrupts]]
source = 124
name = "MODEM_BT_MAC"
instance = "BT_MAC"
output = "PRIMARY"
sources = ["IDF_INTERRUPTS_H"]
[[interrupts]]
source = 133
name = "MODEM_BT_MAC_INT1"
instance = "BT_MAC"
output = "NRT"
sources = ["IDF_INTERRUPTS_H"]

[[unbound-interrupts]]
source = 95
name = "MSPI_FLASH"
reason = "The source covers the flash controller pair; the raising instance is not established."
sources = ["IDF_INTERRUPTS_H"]
```

The BT_MAC type declares `PRIMARY` and `NRT`; both sources bind to that
single instance. Shared acknowledgement banks are type facts. The 32 kHz
selector is shared by two instances, even though MODEM_PWR uses it for two
outputs. Unknown-owner sources remain explicitly unbound with a reason.

`ChipModel::controls()` returns each link's instance, kind, role, sources,
resolved absolute field identity, absolute register address and bit range,
and numeric mux inputs. `gate_groups()` returns resolved fields and sources;
`shared_controls()`, `clock_sources()`, `interrupt_sources()` and
`unbound_interrupts()` retain their checked root facts.

### Importing vendor blocks

`chip_import::import` (`cargo registers import --chip <chip>`) writes block
types, the chip root and an evidence catalog from the blocks a chip's
`registers/import.toml` lists: a type per header device layout, its
instances by SVD peripheral (or by `reg_base.h` macro for a block the SVD
lacks). It reads the pinned vendor SVD and the pinned ESP-IDF headers
(`vendor/artifacts.toml`); where they disagree the header wins: offsets,
fields and bits, each field's `class` (with `debug-only` and `read-clears`
from the header's descriptions), defaults and reserved bits. The SVD names
registers and the fields at the header's bits, binds interrupt sources
and states the reset of bits no header field covers. Interrupts are resolved
by name in `interrupts.h`, which supplies their numbers even when the SVD
number differs. An instance's optional `interrupts = { SVD_NAME = "HEADER_NAME" }`
maps differing spellings explicitly; the imported binding cites the SVD and
the target enumerator's line. An unknown identity or unused mapping is an
error. A register the header lacks keeps its SVD fields as
`read-only`; one in a word the header's layout reserves is left out. SVD
arrays whose elements import identically become `dim` registers. Instances
listed as one type must import identically. The strictness overlay's
command registers, ports, `define-fields`, driver writes and values move as
data. The lanes outside `define-fields` are reserved, citing the overlay's
layout evidence along with the header. Every `sources` entry is an evidence
ID of the written catalog: one per pinned file and one per cited line
(`<FILE>_L<line>`, its description holding the `esp-idf:<path>:<line>`
locator). The report,
`target/registers-import/<chip>/report.md`, lists every disagreement and
what the import made of it. The import writes a chip's first types and
root; from then on those files, with the curated refinements merged into
them, are the source of truth, and the import is not run over them again.

The root covers every source in the header's `0..ETS_MAX_INTR_SOURCE`:
existing SVD bindings name imported instances, and every other source is
explicitly unbound with a reason. Names remove `ETS_` and either
`_INTR_SOURCE` or `_SOURCE`, preserving embedded `INTR` segments. Each
enumerator, including `ETS_MAX_INTR_SOURCE`, has its own line evidence ID;
the root's source count cites all those lines. Missing bounds, gaps and
duplicate source identities are errors. For S31, unbound reasons name the
unresolved flash/PSRAM controller pair and the PMT/PMU alias conflict.

## Analog-I2C space

The chip manifest's `indirect-spaces = ["analog/phy.toml"]` selects one
independent `schema = 1`, `kind = "analog-i2c"` document. Its `[[blocks]]`
have a unique `name`, byte `code`, cited `sources`, optional `routes` and
explicit sparse byte `registers`. There is no CPU base, pointer conversion
or inferred 256-register array. A route has `profile`, original
`request-code`, `host` (0/1), one-hot-or-zero `read-select`,
`read-mask-bits` (24/32) and sources. Its command code is the enclosing
physical block's code. Thus two C5 request routes 0x62/0x65 reach the same
physical 0x65 bytes while retaining different host and selection facts.

A byte register has `name`, `index`, optional `description`, `access`,
`read-action`, `modified-write-values`, `reset-value`/`reset-known-mask`,
`reserved = { mask, sources }`, `read-safe = { epoch, sources }` and fields.
The optional `read-safe.comparison-mask` selects lanes for the qualified
capture without declaring their meaning or reset state.
Fields use the same `FieldInfo` declarations as MMIO (`name`, `bitOffset`,
`bitWidth`, access, enumerations, read/write actions and constraints).
Inheritance and arrays require explicit expanded field geometry in this
first transport model; unsupported extension keys are errors.
Sourced `views` select strict subranges of a declared parent field using
`name`, `field`, `bit-offset`, `bit-width` and `sources`; they do not duplicate
the parent's hardware declaration or change bit states.

`[blocks.review]` supplies a whole-record review default. `[[review]]`
records replace it for a fully qualified `BLOCK.REGISTER.FIELD` or
`BLOCK.REGISTER`. They use the common provenance, accuracy, completeness
and naming rules. An opaque byte is `BYTE_<two-digit-index>_OPAQUE`;
opaque fields use the usual `BIT_n_OPAQUE` / `BITS_low_high_OPAQUE` names.
Every publication review and every block, route, reserved and safety source
is validated against the chip's evidence catalogs.

Both MMIO and analog geometry expose `BitStates`: described, opaque,
reserved and absent. Gaps never become reserved. Unknown reset is absence
of both reset properties; a partial reset supplies only its nonempty known
mask, with zeroes outside that mask carrying no knowledge. Readable access
does not qualify a dump: `read-safe` additionally needs a finite epoch and
sources, and rejects destructive/modified reads. The current real models
claim no known reset. The S31 model qualifies only the existing 40-byte R1
image in its calibrated-radio epoch, with explicit comparison masks. These
masks do not promote unknown data lanes to described or OPAQUE.

`AnalogAddress { block, index }` is a separate host type. Semantic IDs retain
the existing grammar: `register:<chip>/analog-i2c/0xBBII/8`, where BB/II encode
block/index, and `register-field:<chip>/analog-i2c/0xBBII/8/<offset>/<width>`.
This number is a transport identity; it is never supplied to the CPU memory
map. CMSIS-SVD exports only direct registers and its omission summary records
the indirect spaces. `cargo registers inspect-analog --manifest
chips/<chip>/registers/publication/registers.toml` queries full geometry,
routes, reviews, bit states and semantic IDs without hardware access.

The ordinary publication command writes the `[outputs].analog` destination,
`pac/src/analog_generated.rs`, containing the closed PAC's `analog_i2c` module:
addresses, bounded byte fields, original-request route lookup, register facts
and a separately qualified `READ_SAFE` list. Shared value types live in
`chips/espressif/analog::model`. Sampled values preserve all other bits and
reject another register's field or a value exceeding its width. Unknown or
partial reset never creates a whole-byte reset image. These are pure values;
no analog bus transaction is generated here. Both production PACs compile
this vocabulary; PAC/HAL/PHY and probes consume its fields, physical byte
identities and routes. `Request` retains an original request code separately
from its physical address, preserving C5 alias routing. Configuration payloads
remain algorithms in PAC code and do not assert a hardware reset. Sourced
`views` select strict subranges of declared hardware fields without overlapping
field declarations or changing their bit states. `CAPTURE` contains only
read-safe bytes with an explicit comparison mask, in physical address order.

## The vendor SVD

`render_soc` renders every peripheral of a chip's vendor SVD as a typed
register block in the PAC API's `soc` module. The vendor SVD names fields
and an access, but not what a read or a write does: a write-one-to-clear
status, a field the hardware changes under a read-modify-write and a
reserved lane all read `read-write` there. The pinned ESP-IDF headers state
it, so they decide the API. The overlay's `[[headers]]` table names each
vendor peripheral's `*_reg.h` (offsets, field bits, access classes,
descriptions) and `*_struct.h` (the device layout); the generator reads them
as pinned artifacts of the chip's `artifacts.toml` (`cargo pins fetch`) and
finds every vendor register in its header by name and offset. From the
header (`soc_headers.rs`):

- `R/W` is a plain field, `RO`/`R`/`HRO` a read-only one, a `WT` a write
  trigger written only by its trigger (a wider one with its value); a
  register of write triggers alone is write-only, written from zero and
  never read;
- a register whose every field is write-one-to-clear (`WTC`, `W1C`) is
  sampled and acknowledged with exactly that sample, never modified;
- a writable field the hardware also changes (`R/W/SC`, `R/W/WTC`,
  `R/W/SS`, ...), a set-only one (`W1S`, `WTS`, `WS`, `R/W1`), an unknown
  class and a write-one-to-clear field beside others leave the register
  read-only, except in a command register or a command port's data words:
  that a field clears itself does not say that writing zero to it is
  harmless, so no command is inferred from a class;
- a field the header keeps to internal debugging ("Do not use it in
  applications") is never written: read only, and a write trigger among
  them a lane every write leaves zero, unless the overlay cites the vendor
  driver's write of that field (`driver-writes`, `driver-writes-word`,
  below);
- a field the header describes as `Reserved`, and bits no header field
  declares, are reserved: no accessor, kept by every write;
- a field a read clears ("cleared by a read", "cleared when read",
  `R_SS_RC`) or a port read and written in sequence gives its register no
  API, also when only the vendor SVD's description says so;
- a field the header names otherwise at the same bits takes the header's
  name; the report lists each (`[[renamed]]`);
- a register the header does not define is read-only, and has no API when
  the device layout reserves its word (`uint32_t reserved_<offset>[n]`); a
  peripheral without a header is read-only;
- a register the header places at another offset than the SVD is an error
  until the overlay corrects its `offset`; `add = true` builds a register
  the SVD lacks from its header definition.

A peripheral whose semantics the headers describe only in text (flags that
a read clears or a write acknowledges in `R/W` fields, self-clearing bits)
is stated in the overlay's `[[no-api]]` table with its reason and sources:
none of its registers is rendered until a typed API exists.

The SVD-only rules stay below the header's: a register with undeclared
bits, a write-only field, overlapping fields, vendor modified-write values,
a write constraint, a field without a snake-case name, or a raw interrupt
status (`_RAW`) the header does not make write-one-to-clear is only read; a
FIFO or data port by its name, a register not 32 bits wide, without fields,
sharing a word or whose word the curated model describes is not rendered.

`derivedFrom` peripherals take their base's registers, corrected. The
chip's `model/strictness.toml` ([`StrictnessOverlay`]) also states what the
headers do not, per register (`offset`, `add`, `access`, `reserved`,
`read-side-effect`, `fifo`, `write-one-to-clear`, `raw-status-writable`,
`data-port`: a command port's data words, one field the hardware also
writes, read and stored whole; `define-fields`: the register's fields
where the SVD and its header disagree, each `name`, `offset`, `width` and
`description` from the cited source, replacing the SVD's; each takes the
class of the header field whose bits hold it, and the bits no defined
field holds are reserved; a defined field the header calls reserved
stands only with its driver write; `driver-writes`: per field, the
`esp-idf:` line outside `components/soc/` where the vendor driver writes
it, an assignment to the field of the register's device member or a
register write macro naming the register and the field's macro;
`driver-writes-word`: per field, a value line assigning an identifier and
a store of the whole word, either in one function with the value naming
the field's macro and the store holding the identifier, or, for one bit,
its literal returned by the function (`return`) and a store in the same
file, the path between them listed line by line in `reviewed-chain` and
reported unchecked. The overlay check opens every cited driver line in the
pinned ESP-IDF; a driver write of a field that needs none, or one that
does not hold, fails generation) and per field (`rename`, `access`,
`kind = "plain" | "write-trigger"`, `values`), the command registers
(`[[commands]]`: `fields`, optional `qualifiers`, `sources`) and the
interrupt sources the SVD does not. A command register's `fields` are
one-bit `R/W/SC` or `R/W/WTC` fields its sources state software starts and
the hardware clears when done; its `qualifiers` are such bits a start
writes together with the command, by its argument. The register is written
only by those starts, and each refuses, writing nothing, while any
self-clearing bit of the register reads one (the named ones and every other
one-bit `R/W/SC`/`R/W/WTC` field, held at zero): the zero a start writes
never lands on a running command. Every other field of it is only read.
Every entry
names existing vendor geometry and cites `esp-idf:<path>:<line>` (at the
overlay's `esp-idf` pin, which must be the chip's pinned ESP-IDF) or
`trm:<section>`. An `esp-idf:` citation is checked against the pinned file
(`check_overlay`): a register definition line names the register at its
offset, a field line the field (its words in order) with its bits and an
access class that justifies the entry, a register union's reserved members
the entry's `reserved`, an `interrupts.h` enumerator the source's number;
`ETS_MAX_INTR_SOURCE`, the source count, is no source. A mismatch fails
the generation. The module holds the chip's `Interrupt` enum, one partition
`SocPeripherals` and its `Claims`, which a PAC root claims at once, and a
`const` assertion that no vendor block lists a word of another typed
block. The generated `registers/published/soc.report.toml` lists every
renamed field and every register the pass narrowed, with its reasons.

A typed block with an interrupt source implements
`oer_mmio::InterruptSource` on its interrupt side (or on the whole owner
of a block without sides).

## Direct register model

`oer-registers-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- native unreviewed initialization from explicit peripheral geometry and import
  of CMSIS-SVD standard declarations; the source tool retains original inputs;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities;
- deterministic sparse reviewed-assertion overlays keyed by canonical typed
  chip/address-space/register and register-field identities;
- one explicit sparse `register-identity = "REGION.NAME"` fact for renaming,
  no-op confirmation or materialization in a concrete singular peripheral,
  with retained evidence and applicability;
- retained effective classification, applicability and evidence for every
  applied register, field, access, description and write-semantics claim;
- generic physical-layout and write-semantics invariants;
- reviewed PAC transaction, binding-index and evidence-catalog schemas;
- the chip memory map (`memory::Memory`, `chips/<chip>/registers/model/memory.toml`):
  its address spaces and regions, validated against the model and its owned
  ranges, and its MMIO windows, which the register tool reads.

The raw PAC is generated output only. The PAC pack has no module declarations
for hand-written code; ownership and validation transactions that need raw
register access live in the closed parent PAC.

The closed-PAC transaction pack is schema 5 only. Its
`w1c-register-snapshots` operation binds one non-array 32-bit W1C register field
to an affine sample token. The token has no public constructor, is not cloneable
and is consumed by the exact same-register acknowledgement; no caller-built
clear image crosses this boundary. Sequencing and hardware qualification stay
above the generated raw helper.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The [register tool](../../../chips/espressif/registers/README.md) composes it with
explicitly selected reviewed publication inputs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented by the
[register tool](../../../chips/espressif/registers/README.md).
An absent physical register is created only by one reviewed
`register-identity = "REGION.NAME"` assertion; the
`register-declaration` and `register-name` kinds are rejected with an error. The
top-level manifest is schema 3 and declares the stable chip ID and address
space. Assertion subjects use only canonical `register:<chip>/<space>/...` or
`register-field:<chip>/<space>/...` semantic IDs; other subject spellings are
rejected. The subject must match this model's chip and address
space, have a supported aligned width, fit the named concrete region (and its
register address blocks when present), and not alias or overlap existing
geometry. Identity application is atomic and rejects arrays, clusters,
derived regions, placeholders and noncanonical SVD identifiers. Generated
observations are never consulted for access or modified-write semantics; those
require their own explicit reviewed assertions.

Reusable `[[review]]` metadata is accepted as reviewed coverage only with a
non-empty source list, all three classification fields, and non-`hint`
provenance. Incomplete metadata remains a navigation hint and cannot close a
publication gate. Array coverage is resolved from the structural SVD template,
not by wildcard-matching expanded names.

## Register bit states

Each register or memory-layout bit has one of four states:

| State | Meaning | Evidence |
| --- | --- | --- |
| Described | Named in an open vendor source, or its meaning is otherwise established | A cited source establishes the name or meaning |
| Opaque | Absent from open sources; a use is observed, meaning unknown | A cited source establishes the use |
| Reserved | A source explicitly marks the bit reserved | The register records a cited reserved mask; no field is published |
| Absent | No use is observed in the examined corpus and no source marks the bit reserved | No field or reserved annotation is published |

A use is a write of a value different from reset, an explicit masked set in
a read-modify-write, or a read whose value changes behaviour. Carrying bits
unchanged through a read-modify-write is not a use; neither is writing their
reset value as part of a whole-word store. A whole-word access alone does not
justify an opaque field. Both described and opaque fields require source
evidence (`SOURCE` in the implementation, `sources` in the reviewed model).
Absence is relative to the examined corpus, whose inputs and coverage limits
must accompany a vendor-use check. A use of either a reserved or absent bit
is an error; the checker reports these two cases separately. A bit named in
an open vendor source is never opaque.

Code preserves reserved and absent bits or writes their reset value, and the
PAC exposes no field accessor for them. Layouts follow the same rule: CPU operations
preserve the exact image of unassigned bits, including bits hardware writes.
An existing layout unit can declare `fields = []`: its cited position, size,
whole image and storage remain, with no bit accessors. The `fields` property
is still required. Encoded pointer fields can copy their pointer from another
word image with `with_<field>_from`, preserving all other bits without naming
unassigned lanes.

A whole-register initialization image written by the vendor as an exact
constant remains one cited image operation even where reset is unknown.
The image alone grants no bit accessor and establishes no reset value or
absence of hardware use. Independent masked uses or behaviour-dependent
reads still qualify their own fields. Keep the unresolved lanes in the
unknown-reset report; callers cannot supply an arbitrary replacement image.

A native schema-2 fragment adds one optional property on a register:

```toml
[peripherals.registers.register]
name = "DELAY"
addressOffset = 0
size = 32
reserved = { mask = 0xfffffc00, sources = ["VENDOR_HEADER"] }
```

The nonzero mask must fit the inherited register width and must not overlap
any field. Sources are nonempty, distinct and checked against the publication's
evidence catalogs. Masks above TOML's signed integer range use a hexadecimal
string (up to 128 bits). Array instances inherit the structural register
template's annotation. Sparse reviewed overlays retain it by physical identity
and cannot add an overlapping field. Clean SVD omits these source annotations;
the native model and expanded geometry retain them for checks. No field or PAC
accessor is generated for reserved bits.

## Name origin

A register or field review also records where its name comes from with
`naming`:

| `naming` | Meaning | Requirement |
| --- | --- | --- |
| `vendor` | The vendor's own name | Cites the header, SVD or manual in `sources` |
| `descriptive` | A name this project assigned from established behavior | Published description starts with "Project-assigned name." |
| `opaque` | The meaning is not established | The name ends in `_OPAQUE`; published description starts with "Opaque: meaning not established." |

Every published register and field carries `naming`:
`RegisterModel::load_for_publication`, which publication uses, rejects a model
where one lacks it, while `RegisterModel::load` also opens unreviewed drafts
such as an SVD import. An `_OPAQUE` name and `naming = "opaque"` require each
other, and no published name spells unknown meaning as `UNKNOWN`, `UNNAMED`,
`UNCLASSIFIED` or `OPAQUE` inside the name, so an unreviewed name cannot claim
or hide it. A `descriptive` name states meaning, not position or the vendor's
write: a trailing register offset (`CONFIG_0088`), bit positions alone
(`BITS_5_9`), a write action (`SET_BY_…`, `FORCE_ZERO`, `FORCE_ONE`) or a
complete written image (`IMAGE`, `INIT_IMAGE`) is opaque instead. An opaque
name is its position and nothing else, so it carries no unproven hint of
meaning: a register is `WORD_<offset>_OPAQUE` with its four-digit hexadecimal
offset in its parent, a field `BIT_<n>_OPAQUE` or `BITS_<low>_<high>_OPAQUE`,
and an array inserts `_%s` before the suffix and names its first element's
position. Validation computes the name from the geometry and refuses any
other. The whole content of a register whose meaning is established is its
`VALUE` field. Vendor names keep their
source's spelling. `naming` applies only
to registers and fields. The SVD description carries the marker, so raw PAC
documentation shows it too.
