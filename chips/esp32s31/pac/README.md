# ESP32-S31 radio PAC boundaries

This package owns the restricted radio-register interface.
`RadioPartitions::from_handoff` consumes the platform's `RadioHandoff`, the
proof that every radio singleton was handed over, and returns the radio
registers once as opaque register partitions; register
sets such as `WifiRadioRegisters` and `BluetoothTaskRegisters` are assembled
from those partitions and carry the reviewed domain transactions. Protocol
register sets contain only their protocol's partitions. The radio PHY,
coexistence arbitration and shared baseband partitions, including the BTBB
baseband that Bluetooth and IEEE 802.15.4 both initialize, form the separate
`SharedRadioRegisters` owner; a transaction touching both protocol and shared
registers takes it as an explicit argument, and the coexistence timer bank
belongs to it. Per-protocol coexistence priority registers stay with that
protocol's partition. The PAC holds no protocol route: the HAL owns the
neutral radio root, decides which partitions each route consumes and where
the shared owner lives, retains the others and supplies multi-register
sequencing, polling, delay, recovery and lifecycle policy.

Every handwritten PAC operation is a single transaction: it keeps no state
between calls, has no phases, never polls and never routes. A straight-line
multi-register sequence and the affine receipt it returns (for example a
stopped-scheduler or rollback token) are allowed; pending/step machines,
in-flight flags and retry loops belong to the HAL. There, every poll,
compare or retry-until-stable loop carries an explicit time or attempt budget
and returns a typed error the caller handles when the budget runs out; no loop
is unbounded, even where the vendor's is. A handwritten PAC module has no
loop other than a walk over a fixed-length table. The HAL is the only production crate that depends on this
package.

## Sources and generated outputs

The [source-only publication configuration](../registers/publication/registers.toml)
declares the outputs below. Generation belongs to the host-side
[`cargo registers` publisher](../../../tools/registers/generate/README.md), with shared schemas
and invariants in `tools/registers/model`. Reviewed model, ownership policy and provenance live together under
[`chips/esp32s31/registers`](../registers/README.md). There is no runtime
`pac-gen` dependency or additional generator crate in the production dependency graph.

| Artifact | Source and responsibility |
| --- | --- |
| [Radio SVD](../registers/published/radio.svd) | Published from the reviewed chip register model and project policy |
| [raw/src/lib.rs](../pac-raw/src/lib.rs) | Generated svd2rust register accessors from that SVD |
| [src/generated.rs](src/generated.rs) | Generated semantic capability catalog selected by [PAC API policy](../registers/policy/api.toml), the `layouts` module of the model's memory layouts (`chips/esp32s31/registers/model/layouts/`), re-exported as `oer_esp32s31_pac::layouts`, and the policy's typed register blocks over `oer-mmio` with their partitions and `Claims`, which `RadioPartitions::from_handoff` claims once; `IEEE802154_MAC` (split into its generated interrupt and task sides) is a typed block, its interrupt route word the interrupt matrix's; the eighteen Bluetooth LE blocks of the controller, low-power timer and interrupt partitions are typed blocks too (the interrupt bank names source 124, MODEM_BT_MAC, and the low-power timer source 127; source 133, MODEM_BT_MAC_INT1, samples and acknowledges the same bank, so its one interrupt owner serves both); `WIFI_MAC_INTERFACE_ADDRESS` is the [shared library's block](../../espressif/ieee80211-pac/README.md) at this chip's address |
| [Radio bindings](../registers/published/radio.bindings.toml) | Published register-to-raw-PAC binding metadata |
| [src/ownership.rs](src/ownership.rs) and domain modules | Handwritten authority, register-local operations and restricted access |
| [src/platform.rs](src/platform.rs) | The per-domain handoffs and read-only owners of the typed platform blocks (eFuse MAC words, TRNG, reset causes), and in `platform/usb_serial_jtag.rs` the USB Serial/JTAG console's handoff and its endpoint, interrupt and pad owners |

Both Rust generated outputs are regenerated through the publisher. Do not
edit them by hand or extend their lint exceptions to handwritten files. The
semantic catalog may contain reviewed leaves that no current owner exposes;
that does not authorize a broad register escape hatch.

## Upstream PAC and esp-hal

The separate upstream chain is the pinned `esp-pacs` dependency used by
`esp-hal`. The [`soc` adapter](../../espressif/esp-hal/soc/README.md) owns
typed upstream-register operations, cache/MMU and GDMA transactions. It is
the handwritten `oer-espressif-esp-hal-soc` package. It neither generates
nor replaces this radio PAC.

The two chains use their reviewed dependency revisions. A capability exposed
by either PAC is not itself hardware qualification of a radio operation.

Some owned ranges share 32-bit words with registers ESP-HAL writes. Bits
ESP-HAL reference-counts, the 160 MHz reference gate and every
`MODEM_LPCON.CLK_CONF` gate, are not published here; the radio requests them
through `PlatformClockProvider`. The adopted PSRAM mapping holds a permanent
MPLL reference through ESP-HAL's clock tree (`Psram::from_existing_mapping`),
and the radio holds another while powered, so ESP-HAL never powers MPLL down
through `PMU.HP_ACTIVE_HP_CK_POWER` while the radio writes it. The
remaining shared words rely on ESP-HAL writing them only inside
`esp_hal::init`, before any radio exists. The
[shared-word list](../registers/README.md#words-shared-with-esp-hal)
names each word and its guarantee.

## Validation

`cargo registers generate --check` validates register sources. Publication checks also cover source-only publication
reproducibility unconditionally. Artifact-scoped publication is additionally
checked when its review report is present. Generated addresses, masks and
field positions are not regression test oracles; ownership and memory
protocols are tested through behavior and typed contracts.
