# ESP32-C5

The repository holds no Rust code for the ESP32-C5. The chip is a peer of the
stand: ESP-IDF applications run on it as reference peers for the ESP32-S31.

| Path | Holds | Read by |
| --- | --- | --- |
| `vendor/artifacts.toml` | the pin of the ESP-IDF revision and vendor archives the peers build against | `oer-tools-pins-artifacts` |
| `facts/flash.toml` | the flash map of the peer board: bootloader, partition table, application and OTA data offsets, image header encoding, how a written image starts | `oer-tools-image-build` (the bundle of a catalog image) |
| `facts/partitions/applications.csv` | the peers' partition table | `facts/flash.toml` |

The peers are in [`tools/lab/peers`](../../tools/lab/peers) and
[`tools/vendor/references/esp32c5`](../../tools/vendor/references/esp32c5); the board's
reset behavior is in [hardware errata](../../docs/hardware-errata.md).
