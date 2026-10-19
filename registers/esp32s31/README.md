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
| `published/radio.svd` | Generated portable CMSIS-SVD representation |
| `published/radio.bindings.toml` | Generated binding index |
| `publication/registers.toml` | Source-only publication manifest |
| `model/platform/`, `policy/platform-ownership.toml`, `publication/platform.toml` | The platform publication: the reset-cause register the host reads through JTAG after a power cycle; SVD and `published/platform.bindings.toml` only, no PAC (ESP-HAL owns the block at runtime) |

The generic generator is
[`tools/registers/model`](../../tools/registers/model/README.md).
Its other two checked outputs are
[`pac/raw/src/lib.rs`](../../chips/esp32s31/pac-raw/src/lib.rs) and
[`pac/src/generated.rs`](../../chips/esp32s31/pac/src/generated.rs).
Handwritten runtime ownership and safe hardware access remain in the
[closed PAC](../../chips/esp32s31/pac/README.md), not in publication tooling.

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
cargo registers validate --manifest registers/esp32s31/publication/registers.toml
cargo registers generate --check --manifest registers/esp32s31/publication/registers.toml
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
with the modification notice (see [`NOTICE`](../../NOTICE)).
The reviewed common-PHY `TICK_CONF` carveout is in the radio model; adjacent
platform-owned `MODEM_LPCON` registers remain in the upstream analysis catalog.
See the [PAC provenance map](../../chips/esp32s31/pac/README.md).

Source provenance catalogs retain their exact source IDs, revisions and hashes.
Reserved fields remain absent. A register or field whose meaning is not
established is named by its position alone (`WORD_<offset>_OPAQUE`,
`BIT_<n>_OPAQUE`, `BITS_<low>_<high>_OPAQUE`) and reviewed with
`naming = "opaque"`; see [name origin](../../tools/registers/model/README.md). Neighboring-chip
similarity is not sufficient evidence for an ESP32-S31 address or bit. Reviewed
source assertions, vendor comparison and dated hardware observations retain
their distinct strength. Publication verifies consistency and reproducibility.
