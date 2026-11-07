# ESP32-S31

The chip implementation, from its facts up to the application entry. A
crate's layer is its role in `oer-tools-test-layers`, not its directory.

| Layer | Directory | Holds |
| --- | --- | --- |
| Facts | [`registers/`](registers/README.md), [`facts/`](facts/README.md), `vendor/`, [`knowledge/`](knowledge/README.md) | The register model, chip, module and board facts, vendor pins, hardware-free chip knowledge |
| PAC | [`pac/`](pac/README.md) | Sourced hardware geometry, typed access and one root claim |
| HAL | [`hal/`](hal/README.md) | Owners and hardware atoms: `system`, `power`, `peripherals`, `radio` |
| Platform | [`layout/`](layout/README.md), [`board/`](board/README.md), [`bootstrap/`](bootstrap/README.md) | The address map, the board's settings, cold boot |
| Drivers | [`crypto/`](crypto/README.md), [`radio/phy/`](radio/phy/README.md) | Typical AES; calibration, tracking, retained state and RF lifecycle |
| Service | [`radio/system/`](radio/system/README.md), [`radio/client/`](radio/client/README.md) | One shared-domain actor, client membership and ordered handoff |
| Chip adapters | [`radio/ieee80211/driver/`](radio/ieee80211/driver/README.md), [`radio/bluetooth/driver/`](radio/bluetooth/driver/README.md), [`radio/ieee802154/driver/`](radio/ieee802154/driver/README.md) | Each protocol's port over the HAL: DMA, interrupts and lower MAC |
| Facade and composition | [`radio/ieee80211/station/`](radio/ieee80211/station/README.md), [`runtime/`](runtime/README.md) | The Wi-Fi station; the image's chip platform and retained faults |
| Application entry | [`app/`](app/README.md) | Boot, interrupt layout, executors and service tasks behind `oer` |

`hal/src/radio/` holds the radio's atoms; `radio/` holds the crates above
them. [Coexistence](../family/coex/README.md) policy is the family's.

Exclusive root ownership and inactive route selection do not establish safe
active protocol switching. PHY maintenance admission, protocol power-save
policy and physical RF/clock shutdown have separate owners. Standalone
protocol paths do not establish joint-radio arbitration or RF qualification.
