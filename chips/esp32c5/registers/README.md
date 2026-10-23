# ESP32-C5 radio register ownership

The editable hardware source is the schema-3 [`model/device.toml`](model/device.toml)
and its schema-2 peripheral fragments. It follows the layout of the
[ESP32-S31 register project](../../esp32s31/registers/README.md); the generator is
[`tools/registers/model`](../../../tools/registers/model/README.md).

Register and layout bits follow the [shared bit-state rule](../../../tools/registers/model/README.md#register-bit-states).

A cited exact whole-register initialization image remains one closed image
operation when reset is unknown. The image alone gives its lanes no bit
accessors and supplies no reset value. Sparse MMIO use facts in
`evidence/mmio-use.toml` retain their source masks and conservative indexed
qualifications; their unexposed PAC partition grants no production operation
or read-safe snapshot.

| Path | Owner and purpose |
| --- | --- |
| `model/device.toml`, `model/peripherals/` | Reviewed hardware register geometry and semantics |
| `model/memory.toml` | MMIO regions |
| `model/reviewed.toml` | Typed reviewed assertions with their applicability and evidence |
| `policy/api.toml` | Closed-PAC ownership partitions and typed transactions selected by the publication |
| `policy/ownership.toml` | Shared publication scope of named MMIO ranges |
| `policy/lints.toml` | Reviewed register-model lint policy |
| `evidence/` | Source identities of the reviewed facts |
| `published/radio.svd`, `published/radio.bindings.toml` | Generated SVD and binding index |
| `publication/registers.toml` | Source-only publication composition |
| `model/platform/`, `policy/platform-ownership.toml`, `publication/platform.toml` | The platform publication: the reset-cause register the host reads through JTAG after a power cycle; SVD and `published/platform.bindings.toml` only, no PAC (ESP-HAL owns the block at runtime) |

The model publishes, for chip revision v1.0, the IEEE 802.15.4 MAC aperture
at `0x600A3000` with its interrupt route, the analog I2C master, the shared
modem clock, system and ETM registers, the PCR, PMU, LP AON and SAR ADC
radio fields, the PHY baseband configuration and the Wi-Fi MAC fragments
below. The IEEE 802.15.4 geometry comes from the ESP32-C5
`ieee802154_struct.h` and the common LL pinned in
[`chips/esp32c5/vendor/artifacts.toml`](../vendor/artifacts.toml),
not from the ESP32-S31 model: the register offsets agree, but the channel and
power codes, the event set, the PTI fields, the diagnostic counters and the
transmit-on delays differ.

The Wi-Fi MAC fragments (`model/peripherals/wifi-mac-*.toml`) keep the
peripheral, register and field names of the ESP32-S31 model. Their addresses
and reviews are derived from it: each `C5_*` source in
`evidence/vendor-wifi-libraries.toml` names the ESP32-C5 archive member of
every function the ESP32-S31 source describes and records that its full body
is identical to the ESP32-S31 one apart from modem addresses (and, where
stated, software structure offsets). Modem offsets are equal except where the
evidence lists a shift; the eight TX queue banks are `0x78` bytes apart
instead of `0x7C`. Such reviews use `provenance = "derived"` and
`completeness = "partial"`. Functions whose C5 bodies differ structurally are
never evidence for these facts, and ESP32-S31 HIL observations are not
evidence for the ESP32-C5. The fragments publish register views only; no ESP32-C5 driver
owns them.

```console
cargo registers validate --manifest chips/esp32c5/registers/publication/registers.toml
cargo registers generate --check --manifest chips/esp32c5/registers/publication/registers.toml
```

The generated outputs are this directory's `published/` files. Do not edit
them directly. No ESP32-C5 PAC crate exists yet: the publication selects
neither `policy/api.toml` nor PAC outputs, so its binding index names no
crate; `policy/api.toml` is kept but checked by nothing. A change that adds
the PAC crates selects both.
