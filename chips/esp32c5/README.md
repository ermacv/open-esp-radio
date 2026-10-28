# ESP32-C5

The repository holds no Rust code for the ESP32-C5. The chip is a peer of the
stand: ESP-IDF applications run on it as reference peers for the ESP32-S31.

| Path | Holds | Read by |
| --- | --- | --- |
| `vendor/artifacts.toml` | the pin of the ESP-IDF revision and vendor archives the peers build against | `oer-pins-artifacts` |
| `platform/flash.toml` | the flash map of the peer board: bootloader, partition table, application and OTA data offsets, image header encoding, how a written image starts | `oer-image-build` (the bundle of a catalog image) |
| `platform/partitions/applications.csv` | the peers' partition table | `platform/flash.toml` |

The peers are in [`firmware/peers`](../../firmware/peers) and
[`firmware/vendor/esp32c5`](../../firmware/vendor/esp32c5); the board's
reset behavior is in [hardware errata](../../docs/hardware-errata.md).
