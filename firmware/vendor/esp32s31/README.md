# ESP32-S31 vendor firmware

ESP-IDF applications that run vendor code on the HIL board, so hardware
cross-checks compare vendor and production behavior on the same device.
Each project is an entry of the stand's firmware catalog (its
`firmware.toml`): `cargo hil firmware build <image>` builds it against the
pinned ESP-IDF and the pinned vendor archives of
[`artifacts.toml`](../../../chips/esp32s31/vendor/artifacts.toml), with outputs and `build.json` in
`target/vendor-firmware/esp32s31/<project>/`, and `cargo fw flash
<image> --device <MAC|PORT> builds, flashes and journals it. Each application uses the HIL
partition table, so the HIL runner flashes it into `ota_0` like a production
image.

| Project | Behavior |
| --- | --- |
| `calibration` | Catalog image `vendor-calibration`. Enables the PHY once with full calibration and no stored calibration data, then prints each reported vendor object as `oer-vendor-calibration <object> <hex bytes>` and `oer-vendor-calibration-end` on the USB-Serial/JTAG console. The reported objects and their sizes come from the pinned archives that define them (`main/CMakeLists.txt`); the object is `phy_param` of `libphy.a`. It then answers `r <hex address>` requests with `oer-vendor-calibration-register <address> <value>`; the host chooses the registers. The calibration cross-check consumes both. |
| `ble-scan-reference` | Catalog image `ble-scan-vendor-reference`. The pinned BLE Controller over VHCI answers `SCAN <ms> [ACTIVE]` (at most 10000 ms) with a passive or active legacy scan at the open scanner's HIL parameters and prints `@SCAN` counts, then four snapshots of the completed scanner item, link state, scan state machine, SCAN_REQ descriptor and buffer, and one BLE MAC register image (`@ITEM`, `@LINK`, `@STATE`, `@TXD`, `@TXB`, `@CTX`, `@REG`). A linker wrap of the scanner's item-end memory-manager call takes the snapshots. `SCANF <ms> <aa:bb:cc:dd:ee:ff> [ACTIVE]` scans with filter policy 1 and that one public address in the filter accept list and also counts reports from other addresses. `FAL` prints the Controller SRAM blocks and BLE MAC words each list command changes (`@FALBLK`, `@FALREG`) and the list size (`@FALSIZE`); `FALST` prints the status and published entry count of each list precondition: absent removal, duplicate addition, capacity and address type (`@FALST`). `SYNC` resets the Controller. |
| `ieee802154-reference` | The ESP32-C5 reference peer application built for the ESP32-S31: ESP-IDF's IEEE 802.15.4 driver behind the peer's line protocol, for on-air comparison with the open driver and for the IEEE 802.15.4 points of the calibration cross-check. Its catalog image is `ieee802154-vendor-reference`. |
| `phy-tracking-reference` | Catalog image `phy-tracking-reference`. Not run on a board: a Wi-Fi station that initializes the network interface and starts Wi-Fi, so its image, its bootloader and the ESP-IDF component archives of the build define every PHY, Wi-Fi, network-stack and logging symbol the vendor archive sections linked by the vendor scenarios reference. |
