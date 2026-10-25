# open-esp-radio

Source-only `no_std` radio stack in Rust for the Espressif ESP32-S31:
Wi-Fi, Bluetooth LE and IEEE 802.15.4, without the vendor's radio binaries.
The ESP32-C5 code lies in the tree outside the Cargo workspace and CI.

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
| `protocols/` | sans-IO protocols: Wi-Fi, Bluetooth LE, IEEE 802.15.4, the radio port, network, HIL ([layout](docs/repository-layout.md)) |
| `portable/` | generic `no_std` support: time, memory, trace, Embassy bindings, the HIL agent core |
| `chips/` | everything Espressif: PACs, HALs, PHY, drivers, staged boot, boards, and each chip's register model, notes and vendor pins |
| `firmware/` | the images for a chip: apps, bootstraps, HIL agents, probes, ESP-IDF peers |
| `tools/` | the developer's machine, by domain: `cargo registers`, `cargo fw`, `cargo stand`, `cargo hil`, ... ([tools](tools/README.md)) |
| `docs/` | architecture, source policy, hardware errata and topic pages |

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
