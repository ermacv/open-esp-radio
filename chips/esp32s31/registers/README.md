# ESP32-S31 register model

The chip's hardware as the repository holds it: block types, their root,
the memory layouts and the ownership policy, each fact with its sources
([format](../../../tools/registers/model/README.md)). `cargo registers
generate --chip esp32s31` renders the [PAC](../pac/README.md)
(`pac/src/generated.rs`) and the images' DMA-window linker fragment
(`layout/linker/dma-window.x`) from it; host tools read it through
`oer_tools_registers_model::Chip::load`.

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

## MAC offload formats

The RX-control timestamp source, beacon-memory mapping and reload selection,
transmitted-BSSID abort, beacon-filter abort and beacon-memory tag fields
have function-derived names. Their sources are in
[`blobray-mac-offload-names.toml`](evidence/blobray-mac-offload-names.toml).
Abort-length units and the tag-offset reset request's self-clear behavior
remain unestablished; the reset lane retains its existing ordinary RMW class.

`LENGTH_CONTROL.TXOP_LIST_SELECT` admits the three list slots the vendor
publishes. The software no-slot sentinel is not a writable hardware selector.
`WIFI_TXOP_ENTRY`, `WIFI_TXOP_LIST` and `WIFI_TXOP_LISTS` describe the
continuation format and its fifteen-entry/three-list geometry. They are
unplaced formats: the aperture at `WIFI_MAC.TXOP_LIST_MEMORY_OFFSET` overlaps
the first six HE MPLEN words, and these formats grant no second MMIO owner
or concurrent-use authority.

`WIFI_BEACON_RX_DESCRIPTOR` describes the twelve-byte descriptor at the start
of the fixed 1-KiB beacon memory. The root records its CPU/APB and DMA bus
views as `wifi-beacon-apb` and `wifi-beacon-dma`; these are two views of the
same backing, not independent allocations or general alias mappings. Its
buffer and next words retain raw address images outside the allocatable
`dma-window`. The descriptor is memory, not a MAC configuration register.
EOF is already one during initialization; neither it nor OWNER establishes
a completion or detach/release predicate for this special descriptor.
The model supplies no beacon-memory access owner or running offload lifecycle.

## MAC receive filters and diagnostics

`RX_CONTROL_FRAME_DUMP` names the nine control-frame classes admitted by
the hardware dump filters. Its low half and the other high-half bits stay
opaque. `RX_ERROR_DUMP` describes only the two words decoded and configured
by the vendor's error-dump routine; the adjacent `WORD_00F0_OPAQUE` remains
an observation without that format. `NRX_ERROR` combines the existing count
with its fixed-error-code selector and enable. Configuration fields retain
the vendor's fresh-read RMW contract; code dictionaries and counter lifecycle
remain unestablished.

The pinned VHT bandwidth-signaling RTS leaf changes bit 18. Bit 22 stays
opaque and is preserved by the HAL initializer. RX-hang timing, the
BSS-color-collision producer gate and CCA
SND20 configuration also have function-derived names. Their evidence is in
[`blobray-mac-rx-controls.toml`](evidence/blobray-mac-rx-controls.toml); timing
units and the internal CCA-byte format remain unestablished.

## MAC collision interrupt banks

Primary `ENABLE/STATUS.TX_QUEUE_COLLISION` is bit 8. The vendor FIQ posts
TX queue collision service, whose task inspects the per-queue collision
bitmap before selecting queues for retry processing. This event alone
does not identify a queue, prove DMA detachment or establish automatic
hardware retries. HE BSS-color collision instead uses secondary INT1 bit
12. The pinned local paths and their coverage limits are recorded in
[`blobray-mac-tx-collision-irq.toml`](evidence/blobray-mac-tx-collision-irq.toml).

The station primary mask remains `0x19a878e0`, with TX queue collision
disabled. HE initialization separately enables INT1 BSS-color; the open
IRQ path does not yet service INT1. The primary mask cannot suppress that
secondary source. On-air consequences have not been measured.

## MAC key material and receive-crypto boundaries

`ENTRY_KEY_WORD_4..7` hold bytes 16..31 of the inspected GCMP-256 and
32-byte TKIP publications. The TKIP tail contains Michael MIC-key material;
its storage alone does not establish hardware MIC computation. The
16-byte CCMP/GCMP setter leaves these words unchanged. The separate entry
clear operation zeros all ten words, as the open CCMP path does before
installation. The source and concrete execution boundaries are in
[`blobray-mac-execution-boundaries.toml`](evidence/blobray-mac-execution-boundaries.toml).

The STA/AP management-RX indicator selects framing/key-metadata handling
in its inspected consumer. The separate `ieee80211_ccmp_decrypt` path
invokes a configurable CCMP callback. This does not prove that the indicator
controls callback execution, that its AES backend runs entirely on the CPU,
or a quantified CPU saving. Selector-two bits 16/17 remain opaque.

The ordinary BA restore leaf expects three software arrays indexed by
hardware index. Its captured multicast caller passes three scalar pointers;
the isolated active-index-one case reads unrelated stack data. The
vendor-only characterization runs with:

```console
cargo vendor-check scenario --chip esp32s31 wifi-ba-contract --linker path/to/ld.lld --output target/mac-ba-contract
```

Success means the pinned conditional observations match, including one
explicit unknown-data stop. It is not a production equivalence or an on-air
fault claim. The scenario needs only the pinned libpp/libnet80211 and the
selected linker; it requires no production or vendor firmware image.

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
