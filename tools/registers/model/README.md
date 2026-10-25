# Register model library

## Block types and the chip root

`chip_model::ChipModel::load(root, evidence)` reads a chip as one root file
and the block type files it includes; `Chip::load(checkout, chip)` loads
`chips/<chip>/registers/chip.toml` with every evidence catalog of its
`evidence/` directory, and `Chip::policy()` its ownership policy
`policy.toml`. `cargo registers generate` renders the PAC from them
([generate](../generate/README.md)).

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

Hardware pointer and complete value-domain facts belong to the field in its
block type. Ownership policy names these facts; it carries no second encoding
or value list. A domain narrower than an entire field's representable range
remains in policy/HAL.

```toml
[[registers.fields]]
name = "HEAD"
bits = "19:0"
class = "plain"
pointer = { points-to = "DMA_DESCRIPTOR", placement = "dma-window", encoding = { width = 20, shift = 2 }, sources = ["DMA_HEADER"] }

[[registers.fields]]
name = "PRIORITY"
bits = "23:20"
class = "plain"
value-range = { min = 0, max = 15, sources = ["DMA_HEADER"] }

[[registers.fields]]
name = "ENABLED"
bits = "24"
class = "plain"
values = [
    { name = "DISABLED", value = 0, sources = ["DMA_HEADER"] },
    { name = "ENABLED", value = 1, sources = ["DMA_HEADER"] },
]
write-values-only = true
```

A `pointer` targets a root-included word or array layout (`points-to`) or,
without `points-to`, raw byte storage in its `placement` RAM region. The
region's address and size live only in root `[[ram-regions]]`. A layout target
is placed in that region alone. Optional `member-offset` lies inside it.
`encoding.width` equals the field width; `shift` omits aligned low address
bits. Their sum is at most 32. Target alignment and member offset preserve
all omitted low bits. The region fits one fixed high-address prefix, obtained
from its base, and a 32-bit address space. Thus a BLE word index uses width
20 and shift 2 without repeating an SRAM prefix. A full pointer uses width
32 and shift 0. A raw buffer pointer has shift zero and no member offset.

`low-bits = true` identifies an unshifted partial address (width below 32).
Its optional `high-field = "REGISTER.FIELD"` names a field in the same type
supplying the remaining high bits, as with the Wi-Fi RX descriptor window.
The high field is readable and, for a writable pointer, plain. The Wi-Fi TX
queue head instead obtains its prefix from the placement region. Pointer
fields are plain, write-only or read-only and have no numeric domain or write
constraint; a read-only pointer describes an observation, not publication.

`value-range` is an inclusive hardware domain for reads and writes, including
read-only fields. Its bounds fit the field and it has its own sources. Named
encodings use the existing `values`; each value may cite its own sources,
otherwise the field's resolved review supplies them. `write-values-only`
restricts writes to those encodings. Existing `write-range` constraints and
named values must lie inside a declared `value-range`.

Type-local `[[interrupt-pairs]]` state `status`, `clear` and `sources`.
Both register keys exist and have matching widths and repetition; every
status lane is a lane of the clear word, which may hold more (an event the
caller consumes elsewhere); status reads without side effects and clear
holds only W1C or write-trigger fields. An interrupt snapshot of the policy
takes its clear word from its status register's pair. A status has one clear; several status views may share a clear word.
For example:

```toml
[[interrupt-pairs]]
status = "IRQ_STATUS_0"
clear = "IRQ_CLEAR_0"
sources = ["BT_IRQ_SOURCE"]
```

Loading validates these facts even for a type with no instance or no policy
operation, reporting the type file and field/pair that breaks the rule.

A pointer's `high-field` takes the fixed high bits of its placement
(`base >> width`) and nothing else: it has no `values`, `value-range`,
`write-range` or `write-values-only`, and every pointer naming it agrees on
the prefix. The PAC sets it only to that prefix.

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
| `self-clearing` | `R/W/SC`, `R/W/WTC` | getter only, unless a `[[commands]]` or `[[doorbells]]` entry names it |
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
- `[[doorbells]]` (`register`, `field`, `sources`): a one-bit
  `self-clearing` field outside a command register that software sets to
  request one hardware action and the hardware clears when it accepts it.
  Its register's other fields are `plain`, `read-only` or `write-trigger`,
  and it is no FIFO, data port or word a read changes. A read-modify-write
  of the register writes the doorbell back as read, as the vendor's own
  read-modify-writes do (the source says so); the PAC sets it only by its
  trigger;
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
are rejected. Gates require one `plain` bit; resets require one bit of
class `plain`, `write-only` or `write-trigger` (a reset pulse); a mux
requires a `plain` field. A field has one control kind across all instance
links and gate groups: clock gate, reset or mux. Roles are nonempty segments of ASCII
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

### Ownership policy (`schema = 6`)

`chip_policy::ChipPolicy::load(path, model, evidence)` reads the ownership
policy over a loaded `ChipModel`.

The policy holds ownership and transactions only. A **part** is the words
of one MMIO instance one owner holds; a **partition** is one owner, a
generated owner type holding parts of any number of instances, with no
limit on the parts of one instance. Every word (an expanded element: a
register, a `dim` element or a group element) of every MMIO instance is
held by exactly one part or is explicitly unclaimed: nothing claims "the
rest" of an instance or of the chip. Analog-I2C instances are reached
through their I2C master's words and are not partitioned.

```toml
schema = 6
chip = "esp32s31"                 # the root's chip

[[partitions]]
name = "Ieee802154Peripherals"    # owner type, UpperCamelCase
member = "ieee802154"             # its member in the generated root
description = "The IEEE 802.15.4 MAC and its event channels."

[[partitions.parts]]
name = "Task"                     # UpperCamelCase, unique in the partition
instance = "IEEE802154"
registers = ["CONFIG", "ED_CONFIG", "RX_STATUS", "TX_STATUS"]

[[partitions.parts]]
name = "Irq"
instance = "IEEE802154"
registers = ["EVENT_STATUS", "EVENT_CLEAR"]
observes = ["RX_STATUS", "TX_STATUS", "ED_CONFIG"]
irq-side = true
description = "The handler samples and acknowledges events and reads the RX and TX status."
sources = ["IDF_IEEE802154_LL"]

[[partitions.parts]]
name = "Etm"
instance = "MODEM_ETM"
registers = ["CHANNEL_ENABLE"]
elements = [{ register = "CH%s_EVENT", first = 0, count = 2 }]
triggers = [{ register = "CHANNEL_ENABLE_SET", fields = ["CH0", "CH1"] }]
description = "Event channels zero and one."
sources = ["IDF_IEEE802154_ETM"]

[[partitions]]
name = "SystemTimerPeripherals"
member = "system_timer"
description = "The system timer."

[[partitions.parts]]
name = "Counter"
instance = "SYSTIMER"
registers = ["UNIT0_VALUE_HI", "UNIT0_VALUE_LO", "UNIT0_OP"]
shared = true
description = "The counter latch every context reads."
sources = ["IDF_SYSTIMER"]

[[unclaimed]]
instances = ["UART1", "UART2"]    # whole instances
reason = "No driver uses them."

[[unclaimed]]
instance = "MODEM_ETM"            # or words of one instance
registers = ["CH3_EVENT"]
triggers = [{ register = "CHANNEL_ENABLE_SET", fields = ["CH3"] }]
reason = "Event channel three is unused."

[[fixed-register-images]]
name = "enable_ieee802154_etm_channel0"
register = "MODEM_ETM.CHANNEL_ENABLE_SET"
value = 0x00000001
sources = ["IDF_IEEE802154_ETM"]

[[field-replace-modifies]]
name = "set_ieee802154_channel"
fields = [{ field = "IEEE802154.CONFIG.CHANNEL", source-bit-offset = 0 }]
domain = "Channel"
sources = ["IDF_IEEE802154_LL"]
```

A part selects words of its `instance` by `registers` (element names
below the instance, `STATUS`, `REGCLK_1`, `T1.CONFIG`, or a template key
holding `%s`, `CH%s_EVENT`, which selects every element), `elements`
(`count` elements of one template from index `first`) and `triggers`. Only
a word whose fields are all one-bit `write-trigger` fields is split by
field (`triggers`): each of its fields is then held by exactly one part or
unclaimed. Field-level ownership of any other word does not exist. An
`[[unclaimed]]` entry takes either `instances` (whole instances, which no
part names) or `instance` with the same selectors, and a nonblank
`reason`.

A part's flags:

| Flag | Meaning | Rule |
| --- | --- | --- |
| `observes` | registers of the instance it reads | each reads without a side effect and is held by another part |
| `irq-side` | an interrupt handler owns it | not `shared`; names its `interrupt` |
| `interrupt` | the root's interrupt source the part serves (its name) | bound to the part's instance; one part per source; the part's owner names the source (`InterruptSource`) |
| `shared` | every context reaches it through `&self` | each word is a side-effect-free read, write-trigger strobes or one `plain` or `write-only` field stored whole; a strobe writes from zero, never a read-modify-write, so a `write-one-to-clear` field beside the strobes takes zero and is only read; no FIFO, data port, command, `read-clears`, and no `write-one-to-clear` field in a word without a strobe; no `observes`, `triggers`, `read-only`, `irq-side`; no transaction |
| `read-only` | the owner only reads its words | each word reads without a side effect; no `triggers`; no writing transaction |
| `copy` | the read-only owner is `Copy` | only with `read-only` |

A part with any flag, `observes` or `triggers` has a `description` and
`sources`. Partitions have unique names and members, a description and at
least one part; a part selects at least one word.

Policy domains (`flag-domains`, `enum-domains`, `bounded-domains`,
`opaque-domains`, `bitwise-composed-domains`) retain operation-specific inputs.
A `[[field-domains]]` entry names `name` (the policy's UpperCamelCase name)
and `field` (`INSTANCE.REGISTER.FIELD`, including templates), with an optional
policy `description`: it refers to the type's `value-range` or `values`
without copying them. `positioned = true` places an enum reference's
values at the field's own bits, for a masked register modify whose input is
exactly that field's lane (for example, the TSENS clock-enable bit at
position 30); the shift is the field's, never a copy. Bounded references
are unpositioned. All transaction validation uses the referenced domain.
An opaque or bitwise-composed domain names its `register` and is used on
it alone. Transactions name `INSTANCE.REGISTER` and
`INSTANCE.REGISTER.FIELD` of the root, a template key for every element
(`MODEM_ETM.CH%s_EVENT.ID`), and take no `peripheral` or `exposure`. The
kinds are `interrupt-snapshots` (`status`, optional `events`), `field-reads`,
`full-register-reads`, `partition-image-reads`, `full-register-writes`,
`pointer-fields`, `zero-based-field-writes` (`fields`),
`sampled-bit-zero-writes`, `zero-register-writes`,
`fixed-register-images`, `fixed-register-sequences` (each step one
element), `masked-register-modifies`, `field-or-modifies`,
`field-clear-modifies`, `field-replace-modifies` and
`indexed-bit-set-modifies`. Names are unique `lower_snake_case` across
all transactions; the fields of one transaction lie in one register.

What a transaction may reach follows the field class and the word:

| Transaction | Fields it may write or read | Its word |
| --- | --- | --- |
| read | not `write-only`, `write-trigger` or `read-clears` | reads without a side effect: no FIFO, read side effect or `read-clears` field |
| write from zero (full, zero-based, sampled bit, pointer, zero register) | `plain`, `write-only`, `write-trigger`; a data port's `hardware-written` field by a full-register write (a pointer: `plain` or `write-only`; a sampled bit: one `plain` bit); a zero-based write's `whole-word-bits` also `read-only` fields and undeclared bits | no FIFO or command register, no `self-clearing`, `set-only` or `write-one-to-clear` field, no `hardware-written` field outside a data port; a data port only by a full-register write; at least one field a write gives a value |
| read-modify-write | `plain` | reads without a side effect; no command register, data port, `write-only`, `write-one-to-clear`, `self-clearing`, `set-only` or `hardware-written` field |
| fixed image or sequence step | sets bits of `plain`, `write-only`, `write-trigger`, `set-only`, `self-clearing`, `hardware-written`; with `whole-word` also undeclared bits and `read-only` fields | no FIFO or command register; at least one writable field even for zero or `whole-word`; W1C only by a paired image with a proof, never a sequence step |

The shapes and inputs:

- a `full-register-write` names its word's one 32-bit field and takes a
  `domain` or `pointer = true` referring to that field's type-level pointer
  fact; a
  `full-register-read` names one element's one 32-bit field and an opaque
  domain or a bounded one of every `u32`; a signed `field-read` reads an
  8-, 16- or 32-bit field;
- a `masked-register-modify`'s preserve, input and set masks are disjoint
  and cover all 32 bits; its domain's values fit the input mask, except an
  opaque domain's, a raw word the mask selects from;
- `field-or-modifies` and `field-clear-modifies` take a bounded domain
  from zero, an enum domain or a fixed `value`; `field-replace-modifies`
  also a bitwise-composed domain, and an `input-transform` (on a domain,
  with a nonzero `retain-mask`, adding or subtracting) truncates to its
  retain mask. The bits the input may set (a fixed value, a bounded
  domain's bits up to its maximum, an enum's values, a composed output
  mask, a transform's retain mask) lie in the input bits the projections
  take;
- `indexed-bit-set-modifies` take a bounded domain from zero or an enum
  domain whose every index lies below the field's width;
- a `zero-based-field-write` may store fixed ones outside its fields,
  `whole-word-bits = { value, sources }` (a vendor's store of a whole
  word): nonzero, disjoint from its fields, only bits of `read-only`
  fields or of no field, each field's value one its constraint admits,
  with their own sources. The bits grant no setter and no
  read-modify-write; no other kind takes them.

A policy `pointer-fields` entry contains only `name`, `field` and `sources`:
a typed pointer-image operation referencing the field's hardware fact. It
names a plain or write-only pointer. Neither this entry nor a full-register
write may redeclare `points-to`, `placement`, `encoding` or `member-offset`.

Every hardware pointer field is protected, including those with no pointer
operation in policy. It is set only from a typed reference image, by a
`field-or-modifies` or `field-replace-modifies` of one field from input bit
zero with neither `domain` nor `value`, a `zero-based-field-write` of its
image, or to zero. A field exposed by a full-register pointer write is set
only by that write or to zero by a write from zero. No fixed image, masked
modify, indexed bit set, sampled bit or integer full write may forge either
kind of address. Template and element spellings denote the same lanes.

A field's `write-range` or `write-values-only` bounds every transaction:
every lane of a fixed image or step, and the zero a write from zero
leaves in its other fields, is a value its field takes. A snapshot's
acknowledgement may write any subset of its sampled mask and consumed
fields, including zero for retained events and lanes outside them; these possible clear-field
values must all satisfy the field's constraints. A
`full-register-write` takes a bounded domain inside the range or an enum
of taken values, and a `field-replace-modify` a fixed value or, from input
bit zero of its one projection and untransformed, such a domain; a
`zero-based-field-write` takes a field with named values only,
not a narrower range. Every other transaction on a constrained field
fails.

A `write-one-to-clear` field is written only by its snapshot's
acknowledgement. A snapshot names its `status` element; its clear element
is the one the status register's type pairs with it (`[[interrupt-pairs]]`,
at the status element's index), or the status word itself when it holds only
`write-one-to-clear` fields; any other status is refused. Its status word
reads without a side effect, its clear word is one element of
`write-one-to-clear` or `write-trigger` fields (the vendor's `INT_CLR`
strobes) that acknowledge whole readable status fields; a `write-trigger`
clear field lies wholly inside the sampled bits or wholly outside them.
`events` (`INSTANCE.REGISTER.FIELD` of the status word) narrows the sample
to those fields, each acknowledged by the clear word (the console's
USB Serial/JTAG events). A snapshot may name
`consumed-event` (the event the caller consumed elsewhere) with
`consumed-fields`, one-bit `write-one-to-clear` or `write-trigger` fields
of its clear word: the acknowledgement writes the sample and the one
consumed field the caller selects, and every lane it may write satisfies
its field's constraints. The part holding the clear
word takes the snapshot and holds or observes the status word. No other
transaction writes a clear word or a `write-one-to-clear` field except a
fixed image naming one proof, `masks` (an enable word of `plain` fields of
the same part, holding a field for every bit the image acknowledges,
cleared first) or `consumed-event`. A sequence step never writes a clear
word.

Every transaction except a partition image reaches the words of one part:
a template reference needs every element held by that part, and a write to
a split trigger word reaches the part holding the fields its value sets,
with no bit outside them, `whole-word` or not. A partition image reads
every readable word the partition's parts hold, each without a side
effect, and belongs to the partition, not a part. A transaction on an
unclaimed word or field, or on words of two parts, fails. Clock, reset and
mux links and shared controls of the root grant no ownership: the part
holding a control field's word writes it, whichever instances use it.
Every check runs on load and fails with a typed `chip_policy::Error` that
names the file and the entity.

## The thin PAC

`chip_render::render(model, policy)` renders the PAC of a loaded
`ChipModel` and `ChipPolicy` (`chips/<chip>/pac/src/generated.rs`), over
`oer-mmio` and without `unsafe`; `chip_render::dma_window(model)` renders
the images' linker fragment. The PAC is the exact model: field accessors by
class, value domains, typed pointers, ownership parts, layouts and the
analog-I2C dictionary, and no logic.

### Field accessors

A field's class (the table under
[Block type file](#block-type-file-schema--3)) fixes which accessors it has;
nothing else adds one.

| Accessor | Given to |
| --- | --- |
| `read_<register>(&self) -> <register>::R`, a getter `<field>()` per readable field | a register read without a side effect |
| `modify_<register>(&mut self, change)`; `change` takes `&mut <register>::W`, whose `set_<field>(value)` sets one field | a readable register whose write-back replays nothing: every other bit keeps the value read, every trigger, command bit and zero-only field is zero |
| `write_<register>(&mut self, <required>..., change)`, the same `W` from zero | a register with writable fields and no readable one, or a readable one with `write-only` lanes and only `plain`, `read-only`, trigger and write-one-to-clear fields beside them; the fields no write may leave zero are arguments |
| `trigger_<register>_<field>(&mut self)`, or with `value` for a wider field | each `write-trigger` field whose writes are not zero only, and each doorbell |
| `<register>_<doorbell>_pending(&self) -> bool` | each doorbell |
| `start_<register>_<command>(&mut self, <qualifier>: bool, ...) -> Result<(), CommandBusy>` and `<register>_<command>_busy(&self) -> bool` | each command of a `[[commands]]` register |
| `sample_<register>(&self) -> <register>::Snapshot` and `acknowledge_<register>(&mut self, snapshot)` | a register of only `write-one-to-clear` fields |
| `pop_<register>(&mut self)`, `push_<register>(&mut self, value)` | a FIFO port |
| `write_<register>(&mut self, value)` | a data port's word, stored whole |
| `trigger_<register>_<field>(&self)`, `store_<register>(&self, value)` | the strobes and whole-word stores of a `shared` part, per element of an array |

Names are the model's in snake case (`r#` before a keyword). A repeated
register takes `index: oer_mmio::Index<COUNT>` first. A field value is
`bool` for one bit, the native integer for 8, 16 and 32 bits,
`oer_mmio::Bits<WIDTH>` otherwise. A field limited to its `values` by
`write-values-only`, or one whose `values` cover its width, takes its enum
unless a narrower `write-range` bounds it: then it takes a bounded
`<Field>Range`. A field with `values` has a `<field>_variant()` getter. A
field with a `pointer` fact takes `oer_mmio::DmaRef<Layout>` of its target
layout, or the window's `DmaWindowAddress` for a raw buffer, and stores its
encoding (`member-offset` added, shifted, its width kept); a high field has
`set_<field>_prefix()` alone and a write from zero starts from its prefix. A
register a read changes has no accessor, and a field whose class gives no
setter has none.

What a write does with the lanes it does not set:

- a read-modify-write and a readable word's trigger keep the value read in
  `plain`, `read-only`, `debug-only` fields, doorbells, reserved bits and
  the bits of no field, and write zero to triggers, command bits and
  zero-only fields;
- a command start keeps the same lanes and writes zero to triggers,
  write-one-to-clear and set-only lanes; a lane it can neither keep nor
  zero (`write-only`, `hardware-written`, `unknown`) is a render error;
- a write from zero stores the reserved bits at their reset value, each
  high field's prefix and zero elsewhere; a field no write may leave zero
  (a `write-range` above zero, `write-values-only` without zero, a pointer)
  is an argument, and a word with an `unknown` or `debug-only` lane has no
  writer from zero;
- an acknowledgement writes one to each sampled bit and zero to every
  other field, keeping the bits of no field as read now (or at their reset
  value in a clear word that does not read back); a lane whose constraint
  forbids that zero is a render error.

### Owners, names and policy items

- **Modules.** Each MMIO block type is one module holding a register module
  per template (its masks, offsets, `R`, `W`, `Snapshot` and value types)
  and the owners of the parts on its instances. `instances` holds each
  instance's base and bound interrupt sources, and `Interrupt` lists every
  source of the root. `ownership` holds the partitions, `Claims`,
  `partition` and the check that no two claimed blocks share a word.
  `layouts` holds the memory layouts and `analog_i2c` the analog-I2C
  dictionary.
- **Names.** A type's vendor prefix is cut from its register, group and
  field names. The prefix is the longest run of whole `_` segments that
  every named (not `_OPAQUE`) top-level register and group starts with.
  The cut never applies when a type has fewer than two such names, and a
  name that would become empty or start with a digit keeps its prefix. A
  `%s` template whose stem is also a single register's keeps its index
  place as `_n` (`STATUS%s` beside `STATUS` is `status_n`). A part that
  holds every element of an even array gets indexed accessors; one that
  holds only some elements gets one accessor per element, named after the
  element (`ch4_event`, `t1_config`). Two generated names in one Rust
  namespace (a module's types or values, a type's associated items) are a
  render error naming both.
- **Owners.** A part is one owner type: a block of its own, or one part
  of an instance's division when its parts observe or split each other's
  words (two to four parts). `shared` parts reach their words through
  `&self`, and a `read-only` part only reads; with `copy` it is `Copy`. A
  snapshot's clear word is written only by its acknowledgement. A part
  naming its `interrupt` implements `InterruptSource` with that source. A
  part that reaches an 8- or 16-bit word is refused: owners reach 32-bit
  words only.
- **Policy items.** Only the items field accessors cannot express render:
  `whole-word` fixed images, fixed images with a `masks` (masked first,
  whole-word or not) or `consumed-event` proof, interrupt snapshots (with
  the consumed field as an `Option` argument) and zero-based writes with
  `whole-word-bits`. Each becomes a method of the part that owns it; every
  other transaction is HAL code over the accessors.

### Linker fragment

`dma_window` states the root's `dma-window` RAM region
(`DMA_WINDOW_ORIGIN`, `DMA_WINDOW_LENGTH`) and the low address bits its
narrowest low-bits pointer keeps (`DMA_ADDRESS_LOW_BITS`): the link refuses
DMA storage outside the window or at an address whose kept low bits are
zero, the null such a pointer reads.

## Analog-I2C space

Analog-I2C types (`space = "analog-i2c"`) have 8-bit registers addressed
by index; the root's `[[analog-i2c.instances]]` place each by its block
`code` with its `routes` (`profile`, `request-code`, `host` 0 or 1,
`read-select`, `read-mask-bits`, `sources`). A route's command code is its
instance's code, so two request codes may reach the same bytes. An
analog-I2C instance is reached through its I2C master's words and is not
partitioned.

The PAC's `analog_i2c` module is the dictionary of these facts and performs
no access: `blocks` (each code), `Profile`, `route(profile, code)`,
`Request` (an original request code kept apart from the byte it reaches),
a module per instance with a module per register holding its `ADDRESS` and
a `Field` per field and view, `REGISTERS` (each byte's described, opaque
and reserved bits and known reset), `READ_SAFE` (the bytes a finite
reviewed epoch admits to a read) and `CAPTURE` (those with an explicit
comparison mask). The value types are `chips/espressif/analog::model`'s.
`AnalogAddress { block, index }` is a transport identity, never a CPU
address.

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

A type's register states its reserved bits as `reserved = { mask,
sources }` ([Block type file](#block-type-file-schema--3)); no field or
accessor exists for them.

## Name origin

A register or field review also records where its name comes from with
`naming`:

| `naming` | Meaning | Requirement |
| --- | --- | --- |
| `vendor` | The vendor's own name | Cites the header, SVD or manual in `sources` |
| `descriptive` | A name this project assigned from established behavior | Published description starts with "Project-assigned name." |
| `opaque` | The meaning is not established | The name ends in `_OPAQUE`; published description starts with "Opaque: meaning not established." |

Every register and field of a type and every layout entity carries
`naming`; the loader rejects one that lacks it. An `_OPAQUE` name and `naming = "opaque"` require each
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
to registers and fields.
