# Open ESP Radio

`open-esp-radio` exposes the Rust library `oer`. It reexports existing types;
it owns no radio resources, packet queues or executor state.

```rust
use oer::ieee80211::{mac, softmac};
use oer::ieee80211::sta::association::{PhyMode, Preference};
```

The default `wifi` feature exposes the portable IEEE 802.11
protocols and their executor-independent drivers (`ieee80211::services`).
`bluetooth` exposes `bluetooth::hci` and `bluetooth::le::ll`;
`i` exposes the IEEE 802.15.4 contracts. These features build without
a chip or executor. Memory and network value contracts are always available.

Chip selection and protocol backends are separate:

| Feature | Public API |
| --- | --- |
| `esp32s31` | `chips::esp32s31::hal`; no radio driver selection |
| `esp32s31-bluetooth` | Bluetooth protocols and `chips::esp32s31::driver::bluetooth`: the hardware engine |
| `esp32s31-i` | IEEE 802.15.4 contracts and `chips::esp32s31::driver::i`: the MAC engine |

Each backend feature includes its chip and portable protocol feature. No
ESP32-S31 composition is exposed for any radio: the facade provides only the
portable protocols, the HAL and the hardware engines (the old compositions
are kept by the tags `legacy-wifi-2026-10-16` and `legacy-radio-2026-10-16`).
Raw PAC authority requires an explicit restricted dependency.
Cargo features are additive: another dependency enabling `wifi` on the same
facade also enables portable Wi-Fi in the final feature union.

Stack and executor adapters are separate integrations. Availability through
the facade does not establish hardware readiness or concurrent-radio
operation. Bluetooth support retains the limitations of the
[Bluetooth feature map](../hardware/esp32s31/driver/bluetooth/FEATURES.md).

Internal libraries depend on their specific contracts and on `oer-radio-port`
for the radio port lifecycle, using the corresponding `oer-*` Cargo package names. They must
never depend on this facade. Association types are shared with the underlying
IEEE 802.11 encoder; the facade introduces no duplicate enums.

## License

The facade's own source is `MIT OR Apache-2.0`. The crates holding the ESP-IDF
port are `Apache-2.0` only (see [`NOTICE`](../../NOTICE)): `oer-ieee80211-ap`,
`oer-i`, `oer-espressif-i-engine` and `oer-radio-clock`, the
last two through `oer-esp32s31-hal`. A build with `wifi` (the default),
`i`, `esp32s31` or any backend feature is therefore effectively
`Apache-2.0`; only `bluetooth` alone keeps the dual license.
