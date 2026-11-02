# open-esp-radio

Source-only `no_std` radio stack in Rust for Espressif chips, today the
ESP32-S31: Wi-Fi, Bluetooth LE and IEEE 802.15.4, without the vendor's radio
binaries. The stand's ESP-IDF peers run on the ESP32-C5.

**Status.** The tree contains portable protocol cores and runtimes, host
reference models and tests, the S31 register model, PAC, HAL, PHY and drivers
for Wi-Fi, Bluetooth LE and IEEE 802.15.4. Application images, HIL scenarios
and ESP-IDF peers exercise those components; their implemented scope is
documented beside the code. The [architecture](docs/architecture.md) describes
the system and its boundaries.

Not affiliated with or endorsed by Espressif Systems.

## Layout

| Path | Contents |
| --- | --- |
| `tools/` | the developer's machine, by domain: `cargo registers`, `cargo fw`, `cargo stand`, `cargo hil`, ... ([tools](tools/README.md)) |
| `portable/` | generic `no_std` support: time, memory, trace, crypto, Embassy bindings |
| `protocols/` | sans-IO protocols: Wi-Fi, Bluetooth LE, IEEE 802.15.4, the radio port, network, HIL ([layout](docs/repository-layout.md)) |
| `chips/` | the hardware layers: `family/` what every chip shares (staged boot, executor, facts), `<chip>/` its register model, PAC, HAL, PHY, drivers, bootstrap and application entry |
| `oer/` | the application entry (`oer`): what an image uses, the chip selected by the build |
| `examples/` | examples on the entry |
| `hil/` | checks on hardware: images on the entry, probes below it, their services, agent and host programs |
| `docs/` | architecture, source policy, hardware errata and topic pages |

## Build

The toolchain is pinned in `rust-toolchain.toml`.

```console
cargo test
cargo clippy --all-targets
cargo check -p oer-chips-esp32s31-hal --target riscv32imafc-unknown-none-elf
```

The S31 workspace uses its own PAC, HAL, reset entry and staged runtime.
Host `espflash` encodes and flashes images; no esp-* platform crate is linked.

## License

MIT OR Apache-2.0, except the crates holding ESP-IDF ports, which are
Apache-2.0 only. The shared IEEE 802.15.4 MAC vocabulary is one such port
crate; packages depending on it declare Apache-2.0 as their effective license
in Cargo metadata; see [NOTICE](NOTICE) for attribution and third-party material.
