# ESP32-C5 vendor firmware

ESP-IDF applications that run vendor code on the stand's ESP32-C5, so
hardware cross-checks observe the device under test from a vendor
implementation on the air. Each project is an entry of the stand's firmware
catalog (its `firmware.toml`): `cargo hil firmware build <image>` builds it
against the pinned ESP-IDF and the pinned vendor archives of
[`artifacts.toml`](../artifacts.toml), with outputs in
`target/vendor-firmware/esp32c5/<project>/`, and `cargo fw flash
<image> --device <MAC|PORT> builds, flashes and journals it.

| Project | Behavior |
| --- | --- |
| `ble-scan-request-counter` | Catalog image `ble-scan-request-counter`. The pinned BLE Controller over VHCI answers `ADV <ms>` (at most 600000 ms) by advertising a scannable legacy set (`ADV_SCAN_IND`, 30 ms, local name `OERSR` in the scan response) through the extended advertising commands with scan-request notification, then prints `@SCANREQ total=<n>` and one `@SCANNER <address type> <address> <count>` line per scanner. `SYNC` resets the Controller and prints `@READY` and the public address as `@ADDR`. An active scanner's `SCAN_REQ` transmissions are therefore counted on the air independently of its own receiver. |
