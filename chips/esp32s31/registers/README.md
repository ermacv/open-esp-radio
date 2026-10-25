# ESP32-S31 radio register ownership

The editable hardware source is the schema-3
[`model/device.toml`](model/device.toml) and its schema-2 peripheral fragments.
The reviewed assertion overlay in [`model/reviewed.toml`](model/reviewed.toml)
retains explicit applicability and evidence for the IEEE 802.15.4 event-status
identity, access and W1C semantics. Both production generation and vendor
analysis read this model; neither owns a second copy.

| Path | Owner and purpose |
| --- | --- |
| `model/device.toml`, `model/peripherals/` | Reviewed hardware register geometry and semantics |
| `model/memory.toml` | MMIO regions, including platform-owned regions outside radio publication, and the RAM regions memory layouts are placed in (`dma-window`) |
| `model/layouts/` | Memory layouts the hardware reads and writes in RAM: the Wi-Fi RX and TX DMA descriptors, the RX-control prefix and tail |
| `model/reviewed.toml` | Typed reviewed assertions with their applicability and evidence |
| `policy/api.toml` | Schema-5 production PAC ownership partitions and typed transactions |
| `policy/ownership.toml` | Schema-1 shared publication scope of named MMIO ranges |
| `policy/lints.toml` | Reviewed register-model lint policy shared by publication and the investigation |
| `evidence/` | Source identities, provenance, reviewed confidence and supporting records |
| `upstream/platform-radio-deps.svd` | Reviewed upstream platform PAC input for analysis |
| `upstream/esp32s31.svd` | The vendor SVD, unmodified (esp-pacs `a1f6cf68e` base SVD, Apache-2.0); every peripheral the curated model does not describe renders into the PAC's `soc` module |
| `model/strictness.toml` | The strictness overlay over the vendor SVD: each peripheral's pinned ESP-IDF header, from which the generator derives the API, corrected offsets, access, reserved bits, write triggers, command registers, write-one-to-clear, FIFO ports, command data ports and the interrupt sources the SVD lacks, each citing its ESP-IDF header line |
| `import.toml` | The vendor blocks `cargo registers import` writes into block types: each type's ESP-IDF header and its instances' SVD peripherals and `reg_base.h` bases |
| `chip.toml`, `types/`, `layouts/` | The chip as block types and their root (`chip_model::ChipModel` loads them with every catalog in `evidence/`): the vendor blocks `cargo registers import` wrote with the curated refinements merged into them, the curated blocks no vendor header describes (the Wi-Fi MAC, baseband and front end, the Bluetooth blocks, IEEE 802.15.4, coexistence, the modem power block, the CLIC and others, each with a partial extent and its sources), the analog-I2C blocks, every interrupt source bound to an instance or explicitly unbound with a reason, the `dma-window` RAM region and the placement of the memory layouts. These files, not the import, are the source of truth; the import is not run over them again. Generation does not read them yet |
| `policy.toml` | The schema-6 ownership policy over `chip.toml` (`chip_policy::ChipPolicy` loads it): every partition of `policy/api.toml` as parts of instances, every other MMIO word unclaimed with its reason, the value domains and the transactions by `INSTANCE.REGISTER.FIELD`. Generation does not read it yet |
| `evidence/imported.toml`, `evidence/memory.toml` | The evidence of every pinned file and cited line the import named, and of the RAM regions of the root |
| `published/soc.report.toml` | Generated report of every vendor field the API names after its header and every register the conservative pass narrowed, with its reasons |
| `published/radio.svd` | Generated portable CMSIS-SVD representation |
| `published/radio.bindings.toml` | Generated binding index |
| `publication/registers.toml` | Source-only publication manifest |
| `model/platform-device.toml`, `model/platform/`, `policy/platform-ownership.toml`, `publication/platform.toml` | The platform publication: the reset-cause words the host reads through JTAG after a power cycle, from the same `peripherals/lp-aon-reset-cause.toml` fragment the firmware PAC renders; SVD and `published/platform.bindings.toml` only |
| `model/peripherals/{efuse,trng,lp-periclkrst-rng,lp-aon-reset-cause,usb-serial-jtag}.toml` | Platform blocks rendered as typed register blocks (`typed-peripherals` in `policy/api.toml`): the eFuse MAC words, the TRNG, its clock and reset word, both HP cores' reset causes, and the USB Serial/JTAG console's data FIFO, interrupts and pads. Each partition is claimed by its own platform handoff |
| `model/peripherals/core-control.toml` | The core-control blocks, typed and claimed by one platform handoff (`CoreControlPeripherals`): PMU's stall request codes of both HP cores, HP_SYS's stall status, HP_SYS_CLKRST's core 1 clocks and global reset, and LP_AON_CLKRST's software CPU reset of each core; UART0's function clock (`HP_SYS_CLKRST_UART0`) is its own partition (`Uart0ClockPeripherals`) with its own handoff, so the reset restores it without the core-control words |
| `model/peripherals/{timg-wdt,hp-sys-clkrst-timg1,lp-wdt}.toml` | The watchdog blocks, typed and claimed by one platform handoff: both timer groups' main watchdog words (not their timers or RTC calibration words), timer group 1's clock and reset word, the RTC watchdog and the super watchdog (`LP_WDT`, `LP_WDT_SWD`) |

The generic generator is
[`tools/registers/model`](../../../tools/registers/model/README.md).
Its other two checked outputs are
[`pac/raw/src/lib.rs`](../pac-raw/src/lib.rs) and
[`pac/src/generated.rs`](../pac/src/generated.rs).
Handwritten runtime ownership and safe hardware access remain in the
[closed PAC](../pac/README.md), not in publication tooling.

## Publication and investigation are separate

The [source-only project](publication/README.md) selects the reusable chip
provider, model, reviewed assertions, ownership policy and PAC API. It requires
no private artifacts. It explicitly selects the shared lint pack and source evidence catalogs,
without selecting the investigation's executable reconstructions or authenticating
private vendor artifacts.

Evidence sources that describe a board observation in prose
(`evidence/hil-open.toml`, `evidence/hil-vendor.toml`) declare
`kind = "unattested-hil-observation"`: they stay cited until each is
replaced by an attestation bound to a recorded hardware run.

A vendor investigation, which is not part of this repository, selects those
additional inputs explicitly and authenticates artifact-specific facts in its
caller-provided run context. Sharing a publication scope does not
inherit that context or promote a model-only check into comparison evidence.
The common scope is selected through `[registers].ownership-policy`. Its schema-1
pack contains only `owned-ranges`; combining it with an inline `owned-ranges`
list is rejected. A standalone project may instead select an inline scope.
Neither spelling has merge or override precedence.

From the repository root:

```console
cargo registers validate --manifest chips/esp32s31/registers/publication/registers.toml
cargo registers generate --check --manifest chips/esp32s31/registers/publication/registers.toml
```

Omit `--check` only when publishing an intentionally reviewed source change.
Do not edit generated outputs directly. These leaf commands preserve reviewed
model/API validation and independently check each configured output.
The investigation's own publication workflow requires structural review
scopes; an absent review is not silently replaced by model-only publication.

## Shared TSF snapshot words

`WIFI_MAC_TSF_LOAD.CONTROL` publishes separate station and SoftAP
snapshot selectors, but both domains read the same `SNAPSHOT_LOW/HIGH`
pair. `WifiRadioRegisters` serializes selector assertion, both reads
and selector release through its exclusive borrow; the AP driver owner
reaches that transaction through `AccessPointTsf::read` and the HAL.
Separate owners for those selectors
would not provide independent snapshots. The SoftAP selector is TSF-1;
its reviewed sources are in
[`vendor-libpp.toml`](evidence/vendor-libpp.toml) and
[`vendor-net80211.toml`](evidence/vendor-net80211.toml).

The vendor's pre-transmit beacon buffer contains MAC local time, while a
probe-response buffer initially contains zero. Snapshot publication does
not prove that hardware replaces either timestamp in flight, nor which
timer such a replacement uses. That property requires an on-air check.

## Words shared with ESP-HAL

Several owned ranges are single fields of 32-bit words that ESP-HAL also
writes through the upstream `esp-pacs` chain. The two chains do not share a
lock, so a word both write at runtime would lose updates. Every word that a
write transaction of this PAC and pinned ESP-HAL both store to has a reviewed
entry in [`shared-words.toml`](shared-words.toml) that says how the writers
are kept apart: a shared lock, a disjoint lifecycle (ESP-HAL writes only
before any radio owner exists), an ESP-HAL writer that nothing calls, or
whole-word stores.
Words the radio only requests through ESP-HAL, such as `MODEM_LPCON.CLK_CONF`
(reference-counted gates) and `HP_SYS_CLKRST.REF_160M_CTRL0`, are not
published here and have one writer.

## Upstream and evidence boundaries

`upstream/platform-radio-deps.svd` describes official-PAC registers reached by
vendor radio code. It is pinned to the workspace's `esp-pacs` revision and is
not an input to the radio PAC generator. It creates no runtime peripheral owner.
Its `licenseText` is the Apache-2.0 notice of the Espressif SVD it mirrors,
with the modification notice (see [`NOTICE`](../../../NOTICE)).
The reviewed common-PHY `TICK_CONF` carveout is in the radio model; adjacent
platform-owned `MODEM_LPCON` registers remain in the upstream analysis catalog.
See the [PAC provenance map](../pac/README.md).

Source provenance catalogs retain their exact source IDs, revisions and hashes.
Register and layout bits follow the [shared bit-state rule](../../../tools/registers/model/README.md#register-bit-states).

A cited exact whole-register initialization image remains one closed image
operation when reset is unknown. The image alone gives its lanes no bit
accessors and supplies no reset value. Sparse MMIO use facts in
`evidence/mmio-use.toml` retain their source masks and conservative indexed
qualifications; their unexposed PAC partition grants no production operation
or read-safe snapshot.

A register or field whose meaning is not established is named by its
position alone (`WORD_<offset>_OPAQUE`,
`BIT_<n>_OPAQUE`, `BITS_<low>_<high>_OPAQUE`) and reviewed with
`naming = "opaque"`; see [name origin](../../../tools/registers/model/README.md). Neighboring-chip
similarity is not sufficient evidence for an ESP32-S31 address or bit. Reviewed
source assertions, vendor comparison and dated hardware observations retain
their distinct strength. Publication verifies consistency and reproducibility.
