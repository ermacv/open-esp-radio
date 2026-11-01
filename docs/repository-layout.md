# Repository layout

Where code and data live, and how packages are named. The dependency and
execution rules are in [architecture](architecture.md); component READMEs
cover their APIs.

## Roots

| Root | Contents |
| --- | --- |
| `protocols/` | Radio and network formats, ports, cores, runtimes, host models and the HIL protocol and agent |
| `portable/` | Generic support, architectural RISC-V operations and Embassy adapters |
| `sdk/` | Application entries: one crate per chip family, the chip selected by feature |
| `chips/` | Espressif family support, the S31 implementation and chip data |
| `firmware/` | Applications, bootstrap, probes and ESP-IDF projects |
| `hil-scenarios/` | HIL scenarios: the RPC images they share (`images/`), test-only service vocabularies (`services/`) and one host program per scenario |
| `tools/` | Host libraries and programs, grouped by domain; see [tools](../tools/README.md) |
| `docs/` | Cross-cutting documents; see the [index](README.md) |

## Workspace packages

Each path below contains a `Cargo.toml`. Braces list sibling package
folders. This inventory corresponds to `cargo metadata --no-deps`.

```text
chips/esp32s31/bluetooth/driver/
chips/esp32s31/{board, boot, compose, crypto, hal, instrumentation, knowledge,
    layout, pac, phy}/
chips/esp32s31/ieee80211/{driver, mac}/
chips/esp32s31/ieee802154/driver/
chips/esp32s31/radio/{client, system}/
chips/espressif/{analog, boot, coex, efuse, executor, facts, ieee80211-policy,
    ieee802154-engine, ieee802154-mac-values, interrupt-table, modem-clock,
    platform-port, staged-bootstrap, staged-layout, staged-runtime, usb-serial-jtag,
    watchdog}/
firmware/apps/{boot-smoke, radio-root, wifi-station}/
firmware/bootstrap/esp32s31/
firmware/probes/{esp32s31-facts, esp32s31-radio, esp32s31-register, esp32s31-rx-chain}/
portable/{cooperate, crypto, diagnostics, events, memory, mmio, riscv, sort, sync,
    time, time-virtual, trace, wire}/
portable/embassy/{net, time}/
sdk/espressif/
hil-scenarios/{aes, sdk-station, wifi-load, wifi-station}/
hil-scenarios/images/{esp32s31-rpc, esp32s31-sdk-station, esp32s31-wifi-station}/
hil-scenarios/services/{aes, load, sdk-station, wifi-station}/
protocols/bluetooth/hci/
protocols/bluetooth/le/{cipher, controller, ll, model, port, runtime, wire}/
protocols/bridges/ieee80211-ethernet/
protocols/hil/{agent, protocol}/
protocols/ieee80211/{runtime, wire}/
protocols/ieee80211/contracts/{port, security}/
protocols/ieee80211/core/{arbiter, upper-mac}/
protocols/ieee80211/core/extensions/esp-now/
protocols/ieee80211/core/roles/{ap, sta}/
protocols/ieee80211/core/security/{cipher, rsn}/
protocols/ieee80211/testing/{conformance, model}/
protocols/ieee802154/{mac, model, port, wire}/
protocols/network/{port, wire}/
protocols/radio/{coex, port, regulatory}/
tools/analysis/{blobray-analysis, blobray-application, blobray-artifacts,
    blobray-domain, blobray-linker, blobray-riscv, blobray-verification, cli, elf,
    riscv-analysis, riscv-decode, riscv-lift, riscv-model, riscv-program}/
tools/chip/profile/
tools/hil/{cli, harness, link}/
tools/image/{build, bundle, checks, cli, compare, esp-idf, large-values, linker, stack}/
tools/lab/{arbiter, board, cli, client, fixture, fixture-helpers, fixture-install, model,
    router, service}/
tools/os/{command-tree, durable, process, serial, toolchain}/
tools/pins/{artifacts, cli}/
tools/probe/{codegen, macros}/
tools/registers/{cli, contracts, generate, model}/
tools/regulatory/generate/
tools/test/{layers, ui-cases}/
tools/vendor/{cli, ieee802154-esp32s31, ieee802154-esp32s31-cli, lineage,
    provenance, scenario, scenario-cli}/
```

## Data and supporting files

- `chips/espressif/platform/` holds family and ISA fact layers;
  `chips/espressif/notes/` holds shared research notes.
- `chips/esp32s31/platform/` holds chip, module and board facts, partitions,
  ROM functions and stack budgets. `registers/` holds block types, layouts,
  the chip root, evidence and ownership policy. `notes/` holds research
  notes, and `vendor/` holds artifact pins and generated facts. `layout/linker/` holds the chip's linker fragments.
- `chips/espressif/executor/` contains the executor and its `bind!` macro,
  without a chip-specific feature or dependency. `chips/esp32s31/compose/`
  declares the `Esp32s31` executor binding over the HAL's platform port,
  alongside the chip's retained fault records and its platform, console and
  SAR/PWDET bindings.
- `chips/esp32c5/` contains `vendor/artifacts.toml` and `platform/` flash
  metadata and partitions for ESP-IDF peers; it contains no Rust package.
- `firmware/peers/` and `firmware/vendor/` contain ESP-IDF projects, whose
  `firmware.toml` entries form the image catalog.
- `tools/lab/linux-net/` contains the network helper and pinned build inputs;
  `tools/lab/linux-bluetooth/` documents the Bluetooth helper.
- The root `Cargo.toml` and `Cargo.lock` define one Rust workspace.
  `.cargo/config.toml` defines the host command aliases.
  `target/` holds rebuildable output; generated evidence stores and caches
  are described by the tools that own them.

## Names

A library's package name is `oer-` and its path below its root:
`protocols/ieee80211/wire` is `oer-ieee80211-wire`,
`chips/esp32s31/hal` is `oer-esp32s31-hal`, `sdk/espressif` is
`oer-espressif`, `tools/hil/link` is
`oer-hil-link`, and `hil-scenarios/services/aes` is `oer-services-aes`. An
executable keeps its root: `firmware/apps/boot-smoke` is
`oer-firmware-apps-boot-smoke`, `tools/image/cli` is `oer-tool-image-cli`, and
`hil-scenarios/aes` is `oer-hil-scenarios-aes`.
Rust imports replace hyphens with underscores. A program called by name,
such as the image linker or a privileged helper, sets its binary name
with `[[bin]] name`.

## Layer roles

Each package under `chips/`, `protocols/hil/` and `hil-scenarios/` has one
role in `oer-test-layers` that names its level
([architecture](architecture.md#layers)); a directory name such as `driver`
grants nothing. `chips/esp32s31/wifi` assembles the S31 station backend and portable runtime as the Wi-Fi facade.
A protocol leaf named `hil` takes its role from the list. A package under a protocol's
`contracts/` is a protocol contract, which a backend may use, except
`contracts/port`, which keeps the port's role; `testing/conformance` is a
backend conformance suite and `testing/model` a reference model, and no
product package depends on either; and
`protocols/bridges/*` are bridges, which stand only on the codecs they
join. Each `protocols/network/*` package has an explicit layer in
`oer-test-layers`: the network codec stands on `oer-wire` alone and the
network contract on the codec.

## Known deviations

- `protocols/ieee80211/runtime` has a dev-dependency on
  `chips/espressif/ieee80211-policy`.
- `chips/espressif/ieee80211-policy` has a dev-dependency on
  `chips/esp32s31/ieee80211/driver`.
- Normal dependencies outside their role:
  `chips/esp32s31/pac` renders the analog-I2C model types of
  `chips/espressif/analog`; `chips/esp32s31/hal` depends on
  `protocols/radio/coex`; `chips/esp32s31/ieee80211/driver` depends on
  `chips/espressif/staged-runtime` for its trace recorder;
  `chips/esp32s31/ieee802154/driver` depends on `portable/mmio` for
  `Placement`; the HIL RPC and Wi-Fi station images depend directly on the
  HAL instead of a facade.
- HAL production files that wait or poll are listed with exact counts:
  modules of `bluetooth`, `boot`, `ieee80211` and `phy`, and
  `peripherals/entropy.rs`.
- Direct normal dependencies on `embassy-sync` remain in
  `protocols/hil/agent` and `hil-scenarios/images/esp32s31-rpc`.

The layer tests enforce the normal and build dependency exceptions, the
source-file inclusions and the HAL files in their own deviation lists. The
family-to-chip normal/build deviation list is empty. Development
dependencies above are separate from those lists.
