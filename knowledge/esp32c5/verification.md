# ESP32-C5 vendor verification

[`artifacts.toml`](artifacts.toml) is the single pin of every ESP32-C5 vendor
archive and ROM ELF: upstream repository, revision, path and SHA-256 of each.
Fetch and verify them into `target/vendor/<source>/<revision>/` with

```console
cargo verification fetch esp32c5
```

The PHY, Wi-Fi and coexistence revisions are the submodules of the ESP-IDF
revision pinned for the [ESP32-S31](../esp32s31/README.md), so both chips are
compared against the same vendor release. The Controller archive is the
`esp32c5-bt-lib` submodule of that ESP-IDF revision. The ROM ELFs of chip
revisions v0.x (`rom-rev0`) and v1.0 (`rom-rev100`) are pinned separately.
The ESP-IDF files are the IEEE 802.15.4 driver sources and the ESP32-C5
register, SoC and LL headers they include, the counterparts of the ESP32-S31
pin.

| Path | Contents |
| --- | --- |
| `scenarios/` | Typed vendor scenarios: the verdict library `oer-esp32c5-vendor-scenarios` and, in `cli/`, its binary `oer-esp32c5-vendor-scenarios-cli` behind the shared report command line; `phy_i2c` compares the analog-register I2C transport of the pinned `libphy.a` with the production transport, over the v1.0 ROM |
| [`probes/`](probes/README.md) | Compiled Rust entry points of the production code the scenarios run |
| `facts/provenance.toml` | Reviewed code fingerprints of the cited vendor functions, which `cargo verification check provenance --chip esp32c5` holds the pins to |
| [`hardware/register-probe/`](hardware/register-probe/README.md) | A board image that checks the published [register model](../../registers/esp32c5/README.md) on the silicon |
