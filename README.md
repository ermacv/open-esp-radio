# open-esp-radio

Source-only `no_std` radio stack in Rust for the Espressif ESP32-S31 and
ESP32-C5 (ESP32-C6 later): Wi-Fi, Bluetooth LE and IEEE 802.15.4, without the
vendor's radio binaries.

**Status.** Rebuilding. The protocol crates, the register model, PAC, HAL,
PHY and the Wi-Fi driver's DMA and MAC layers are in place and tested on the
host; no radio works on hardware yet. The first goal is a Wi-Fi station,
reached in steps that each show a result on a board (boot and PHY, passive
scan, active scan, open association with traffic, WPA2). IEEE 802.15.4 and
Bluetooth LE follow.

Not affiliated with or endorsed by Espressif Systems.

## Layout

| Path | Contents |
| --- | --- |
| `crates/` | the radio libraries, placed by protocol and chip ([architecture](docs/architecture.md)) |
| `firmware/` | the chip-target workspace: staged boot platform and images |
| `registers/` | the reviewed register model (`cargo registers` publishes its SVD and PAC) |
| `tools/` | host tools: `cargo registers`, `cargo fw`, by layer ([tools](tools/README.md)) |
| `knowledge/` | recovered hardware facts and research notes |
| `docs/` | architecture, owner decisions, source policy, hardware errata |

## Build

The toolchain is pinned in `rust-toolchain.toml`.

```console
cargo test
cargo clippy --all-targets
cargo check -p oer-esp32s31-hal --target riscv32imafc-unknown-none-elf
```

Chip code depends on the owner's esp-hal fork (`ermacv/esp-hal`); upstream
esp-hal does not run the staged boot.

## License

MIT OR Apache-2.0, except the crates holding ESP-IDF ports, which are
Apache-2.0 only; see [NOTICE](NOTICE) for third-party material.
