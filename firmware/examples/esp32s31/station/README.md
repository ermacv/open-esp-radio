# ESP32-S31 station firmware

This is the production-shaped, non-HIL Embassy application for the open radio
driver. It uses the ESP32-S31 PAC-backed PHY/MAC implementation and must not
depend on the HIL protocol, qualification telemetry or benchmark policy.

The application is intentionally a separate target workspace because it uses
the embedded RISC-V standard-library build and ESP-IDF-compatible linker
layout. Run Cargo from this directory so its target and linker configuration
is applied:

```console
cd examples/esp32s31/station
cargo check --release
```

The example uses the owned Xarxa/Embassy stack, the only network
implementation; the
[implementation guide](../../../docs/network-implementations.md) explains its
crates and the shared Wi-Fi boundary.

## Application behavior

Network credentials are application build configuration. They are deliberately
absent from reusable driver crates and HIL configuration:

```console
ESP32S31_WIFI_SSID='your ssid' \
ESP32S31_WIFI_PASSPHRASE='your passphrase' \
cargo check --release
```

The application scans channels 1 to 13 once, then starts a WPA2-Personal
station. Once DHCP completes, it serves UDP echo on port 4321 and replies to
ICMP ping. Socket storage and IP policy live in `src/network.rs`.

The station request's `StaReconnectPolicy` in `src/main.rs` bounds the
attempts and backoff when no matching access point answers or the link is
lost.

Build the complete application from the repository root:

```console
cargo fw build station
cargo fw flash --device <MAC|PORT> --monitor target/firmware/esp32s31-station/build-<id>
```

The [shared platform](../../../platform/esp32s31/README.md) initializes PSRAM,
relocates the separately linked application and keeps DMA and interrupt storage
in SRAM. The build checks ELF placement and stack frames before packaging
the image bundle. `cargo build` in this example produces only the stage-two
ELF; flash the bundle `xtask` built. Hardware readiness still requires
appropriate scenario evidence.

See the [Wi-Fi network integration](../../../docs/wifi-egress.md#owned-tx-path)
for packet ownership, RX scheduling and admission limits, and the
[integration crate](../../../crates/composition/esp32s31/embassy/ieee80211/README.md)
for the station lifecycle across link loss and reconnect; the network stack
and its sockets stay alive while the link goes down and up. Product readiness
is determined by the
[qualification program](../../../qualification/targets/esp32s31/wifi-sta.toml) and
its evidence, independently of this application's source check.
