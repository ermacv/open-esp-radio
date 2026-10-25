# ESP32-S31 register model

The chip's hardware as the repository holds it: block types, their root,
the memory layouts and the ownership policy, each fact with its sources
([format](../../../tools/registers/model/README.md)). `cargo registers
generate --chip esp32s31` renders the [PAC](../pac/README.md)
(`pac/src/generated.rs`) and the images' DMA-window linker fragment
(`layout/linker/dma-window.x`) from it; host tools read it through
`oer_registers_model::Chip::load`.

| Path | What it holds |
| --- | --- |
| `chip.toml` | The root: the types and layouts it includes, every instance at its base with its extent, clocks, resets and selectors, every interrupt source bound to an instance or unbound with a reason, the analog-I2C instances with their routes, the `dma-window` RAM region and the placement of the memory layouts |
| `types/` | The block types: the vendor blocks (each from its pinned ESP-IDF header and the vendor SVD, refined with the curated facts), the curated blocks no vendor header describes (the Wi-Fi MAC, baseband and front end, the Bluetooth blocks, IEEE 802.15.4, coexistence, the modem power block, the CLIC and others, each with a partial extent and its sources) and the analog-I2C blocks |
| `layouts/` | Memory layouts the hardware reads and writes in RAM: the Wi-Fi RX and TX DMA descriptors, the RX-control prefix and tail, the BLE controller structures and the PHY frequency records |
| `policy.toml` | The schema-6 ownership policy: the owners as partitions of parts of instances, every other MMIO word unclaimed with its reason, the interrupt source each handler part serves, operation-specific domains, references to the complete field domains in block types, and the transactions by `INSTANCE.REGISTER.FIELD` |
| `evidence/` | Source identities, provenance, reviewed confidence and supporting records, every catalog loaded with the root |

```console
cargo registers validate --chip esp32s31
cargo registers generate --chip esp32s31 --check
```

Omit `--check` to write an intentional model change's outputs. Generated
outputs are never edited by hand.

## Intrinsic hardware facts

Block fields in `types/` own their sourced `pointer` facts, complete numeric
`value-range` domains and enum `values`. Pointer targets name layouts in
`layouts/`, and placement names the root's single `dma-window` RAM region;
no encoding repeats that region's address or size. BLE compressed pointers
state width 20 and shift 2; full and member pointers state width 32 and shift
0; Wi-Fi RX observations and TX queue heads state `low-bits`, and the RX
descriptor window's `ADDRESS_HIGH` is their high field, which takes the
window's prefix `0x2f0` alone. IEEE 802.15.4 frame pointers target raw
buffer storage rather than an invented descriptor layout.

Interrupt status words name their clear words in their type
(`[[interrupt-pairs]]`): the Wi-Fi MAC's event status and clear bank, the
modem power block's, the USB Serial/JTAG console's, and the Bluetooth
interrupt status and NRT snapshot views that share their W1C clear banks.
The policy's snapshots name only their status; the console's samples its
two console events (`SERIAL_OUT_RECV_PKT`, `SERIAL_IN_EMPTY`).

`WIFI_MAC.RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` is a self-clearing doorbell
(`[[doorbells]]`): rom `wDev_AppendRxBlocks` sets it by a read-modify-write
and waits for the hardware to clear it, and `hal_mac_rx_enable` and
`hal_mac_rx_disable` write it back as read.

The schema-6 policy's `field-domains` references preserve policy names without
copying value lists or bounds; a `positioned` one stands at its field's bits.
Its pointer operations reference hardware fields without redeclaring targets,
encodings or placement. Narrow domains, array indices and split or
transformed operation inputs remain policy.

## Evidence

Evidence sources that describe a board observation in prose
(`evidence/hil-open.toml`, `evidence/hil-vendor.toml`) declare
`kind = "unattested-hil-observation"`: they stay cited until each is
replaced by an attestation bound to a recorded hardware run.

Source provenance catalogs retain their exact source IDs, revisions and hashes.
Register and layout bits follow the [shared bit-state rule](../../../tools/registers/model/README.md#register-bit-states).

A cited exact whole-register initialization image remains one closed image
operation when reset is unknown. The image alone gives its lanes no bit
accessors and supplies no reset value. Sparse MMIO use facts in
`evidence/mmio-use.toml` retain their source masks and conservative indexed
qualifications.

A register or field whose meaning is not established is named by its
position alone (`WORD_<offset>_OPAQUE`,
`BIT_<n>_OPAQUE`, `BITS_<low>_<high>_OPAQUE`) and reviewed with
`naming = "opaque"`; see [name origin](../../../tools/registers/model/README.md#name-origin). Neighboring-chip
similarity is not sufficient evidence for an ESP32-S31 address or bit. Reviewed
source assertions, vendor comparison and dated hardware observations retain
their distinct strength.

## Shared TSF snapshot words

`MODEM_PWR.TSF_LOAD_CONTROL` publishes separate station and SoftAP snapshot
selectors, but both domains read the same `SNAPSHOT_LOW/HIGH` pair: one
owner serializes selector assertion, both reads and selector release.
Separate owners for those selectors would not provide independent
snapshots. The SoftAP selector is TSF-1; its reviewed sources are in
[`vendor-libpp.toml`](evidence/vendor-libpp.toml) and
[`vendor-net80211.toml`](evidence/vendor-net80211.toml).

The vendor's pre-transmit beacon buffer contains MAC local time, while a
probe-response buffer initially contains zero. Snapshot publication does
not prove that hardware replaces either timestamp in flight, nor which
timer such a replacement uses. That property requires an on-air check.
