# Repository layout

Where code and data live, and how packages are named. The dependency and
execution rules are in [architecture](architecture.md); component READMEs
cover their APIs.

## Roots

| Root | Contents |
| --- | --- |
| `protocols/` | Radio and network formats, ports, cores, runtimes, host models and the HIL protocol and agent |
| `portable/` | Generic support, architectural RISC-V operations and Embassy adapters |
| `chips/` | Espressif family support, the S31 implementation and chip data |
| `firmware/` | Applications, bootstrap, HIL images, probes and ESP-IDF projects |
| `tools/` | Host libraries and programs, grouped by domain; see [tools](../tools/README.md) |
| `docs/` | Cross-cutting documents; see the [index](README.md) |

## Workspace packages

Each path below contains a `Cargo.toml`. Braces list sibling package
folders. This inventory corresponds to `cargo metadata --no-deps`.

```text
chips/esp32s31/bluetooth/driver/
chips/esp32s31/{board, compose, hal, instrumentation, instrumentation-hil,
    knowledge, layout, pac, phy, phy-probe}/
chips/esp32s31/ieee80211/{driver, mac}/
chips/esp32s31/ieee802154/driver/
chips/esp32s31/radio/{client, system}/
chips/espressif/{analog, boot, coex, efuse, executor, facts, ieee80211-policy,
    ieee802154-engine, ieee802154-mac-values, interrupt-table, modem-clock,
    platform-port, staged-bootstrap, staged-layout, staged-runtime, usb-serial-jtag,
    watchdog}/
firmware/apps/{b0, boot-smoke, radio-root, w0, w1, w2, w3}/
firmware/bootstrap/esp32s31/
firmware/hil/{esp32s31, esp32s31-lower-mac, esp32s31-phy}/
firmware/probes/{esp32s31-facts, esp32s31-radio, esp32s31-register, esp32s31-rx-chain}/
portable/{cooperate, crypto, diagnostics, memory, mmio, riscv, sort, sync, time,
    time-virtual, trace, wire}/
portable/embassy/{net, time}/
protocols/bluetooth/{control, hci}/
protocols/bluetooth/le/{cipher, controller, ll, model, port, runtime, wire}/
protocols/hil/{agent, protocol}/
protocols/ieee80211/{ap, arbiter, cipher, conformance-control, control, esp-now,
    instrumentation, model, port, rsn, runtime, sta,
    upper-mac, wire}/
protocols/ieee802154/{mac, model, port, wire}/
protocols/{network, network-control}/
protocols/radio/{coex, port, regulatory}/
tools/analysis/{blobray-analysis, blobray-application, blobray-artifacts,
    blobray-domain, blobray-linker, blobray-riscv, blobray-verification, cli, elf,
    riscv-analysis, riscv-decode, riscv-lift, riscv-model, riscv-program, riscv-stack}/
tools/chip/profile/
tools/hil/{analysis, cli, executor, family-lower-mac, family-system, family-wifi,
    fixture, fixtures-bluetooth, lab, link, model, net-traffic, observer, registry, run, runner,
    runs, scenario}/
tools/image/{build, bundle, checks, cli, compare, esp-idf, large-values, linker}/
tools/lab/{arbiter, board, cli, fixture, fixture-install, model, router}/
tools/os/{command-tree, durable, process, serial, toolchain}/
tools/pins/{artifacts, cli}/
tools/probe/{codegen, macros}/
tools/registers/{cli, contracts, generate, model}/
tools/regulatory/generate/
tools/test/{layers, ui-cases}/
tools/vendor/{cli, ieee802154-esp32s31, ieee802154-esp32s31-cli, lineage, phy,
    phy-esp32s31, provenance, scenario, scenario-cli}/
```

## Data and supporting files

- `chips/espressif/platform/` holds family and ISA fact layers;
  `chips/espressif/notes/` holds shared research notes.
- `chips/esp32s31/platform/` holds chip, module and board facts, partitions,
  ROM functions and stack budgets. `registers/` holds block types, layouts,
  the chip root, evidence and ownership policy. `notes/` holds research
  notes, and `vendor/` holds artifact pins and generated facts and PHY
  baselines. `layout/linker/` holds the chip's linker fragments.
- `chips/espressif/executor/` contains the executor and its `bind!` macro,
  without a chip-specific feature or dependency. `chips/esp32s31/compose/`
  declares the `Esp32s31` executor binding over the HAL's platform port,
  alongside the chip's retained fault records.
- `chips/esp32c5/` contains `vendor/artifacts.toml` and `platform/` flash
  metadata and partitions for ESP-IDF peers; it contains no Rust package.
- `firmware/peers/` and `firmware/vendor/` contain ESP-IDF projects, whose
  `firmware.toml` entries form the image catalog.
- `tools/hil/scenarios/` is the executable scenario catalog;
  `tools/hil/acceptance/esp32s31/` documents its acceptance programs.
  `tools/hil/linux-net/` contains the network helper and pinned build inputs;
  `tools/hil/linux-bluetooth/` documents the Bluetooth helper.
- The root `Cargo.toml` and `Cargo.lock` define one Rust workspace.
  `.cargo/config.toml` defines the host command aliases.
  `target/` holds rebuildable output; generated evidence stores and caches
  are described by the tools that own them.

## Names

A library's package name is `oer-` and its path below its root:
`protocols/ieee80211/wire` is `oer-ieee80211-wire`,
`chips/esp32s31/hal` is `oer-esp32s31-hal`, and `tools/hil/run` is
`oer-hil-run`. An executable keeps its root: `firmware/apps/w1` is
`oer-firmware-apps-w1`, and `tools/image/cli` is `oer-tool-image-cli`.
Rust imports replace hyphens with underscores. A program called by name,
such as the image linker or a privileged helper, sets its binary name
with `[[bin]] name`.

## Layer roles

Each package under `chips/`, `protocols/hil/` and `firmware/hil/` has one
role in `oer-test-layers` that names its level
([architecture](architecture.md#layers)); a directory name such as `driver`
grants nothing. `chips/esp32s31/wifi` has the facade role before it exists.
A protocol leaf named `instrumentation` is an observation contract; a leaf
named `hil` takes its role from the list.

## Known deviations

- `protocols/ieee80211/runtime` has a dev-dependency on
  `chips/espressif/ieee80211-policy`.
- `chips/espressif/ieee80211-policy` has a dev-dependency on
  `chips/esp32s31/ieee80211/mac`.
- `firmware/hil/esp32s31` has an optional, RISC-V-only normal dependency on
  `firmware/apps/w3` for its `wifi-station` feature.
- Host tests include image source files through `#[path]`:
  `tools/image/cli/src/main.rs` includes the W1 CRC implementation;
  `tools/vendor/phy/tests/` includes cache and hold modules of
  `firmware/hil/esp32s31-phy`.
- Normal dependencies outside their role:
  `chips/esp32s31/pac` renders the analog-I2C model types of
  `chips/espressif/analog`; `chips/esp32s31/hal` depends on
  `protocols/radio/coex`; `chips/esp32s31/ieee80211/driver` depends on
  `chips/espressif/staged-runtime` for its trace recorder;
  `chips/esp32s31/ieee802154/driver` depends on `portable/mmio` for
  `Placement`; the three
  HIL images depend directly on the HAL, drivers, chip adapters, the radio
  system, chip facts, family policy, protocol crates and `portable/riscv`
  instead of a facade.
- HAL production files that wait or poll are listed with exact counts:
  modules of `bluetooth`, `boot`, `ieee80211` and `phy`, and `platform.rs`.

The layer tests enforce the normal and build dependency exceptions, the
source-file inclusions and the HAL files in their own deviation lists. The
family-to-chip normal/build deviation list is empty. Development
dependencies above are separate from those lists.
