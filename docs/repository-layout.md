# Repository layout

Where code and data live, and how packages are named. The dependency and
execution rules are in [architecture](architecture.md); component READMEs
cover their APIs.

## Roots

| Root | Contents |
| --- | --- |
| `tools/` | Host libraries and programs, grouped by domain; see [tools](../tools/README.md) |
| `portable/` | Generic `no_std` support for host and chip, architectural RISC-V operations and Embassy adapters |
| `protocols/` | Radio and network formats, ports, cores, runtimes, host models and the HIL protocol and agent |
| `chips/` | The hardware layers: `family/` what every chip of the family shares, `<chip>/` one chip's register model, PAC, HAL, PHY, drivers, compositions, bootstrap and application entry (`app`) |
| `oer/` | The application entry an image uses (`oer`), its chip selected by the build |
| `examples/` | Examples on the application entry |
| `hil/` | Checks on hardware: images on the entry (`images/`), probes below it (`probes/`), test-only service vocabularies (`services/`), the HIL agent on the entry (`agent/`) and one host program per scenario (`scenarios/`) |
| `docs/` | Cross-cutting documents; see the [index](README.md) |

## Workspace packages

Each path below contains a `Cargo.toml`. Braces list sibling package
folders. This inventory corresponds to `cargo metadata --no-deps`.

```text
tools/analysis/{blobray-analysis, blobray-application, blobray-artifacts,
    blobray-domain, blobray-linker, blobray-riscv, blobray-verification, cli,
    elf, riscv-analysis, riscv-decode, riscv-lift, riscv-model, riscv-program}/
tools/chip/profile/
tools/hil/{cli, harness, link}/
tools/image/{build, bundle, checks, cli, compare, esp-idf, large-values, linker,
    stack}/
tools/lab/{arbiter, board, cli, client, fixture, fixture-helpers,
    fixture-install, model, router, service}/
tools/os/{command-tree, durable, process, serial, toolchain}/
tools/pins/{artifacts, cli}/
tools/probe/{codegen, macros}/
tools/registers/{cli, contracts, generate, model}/
tools/regulatory/generate/
tools/test/{layers, ui-cases}/
tools/vendor/{cli, ieee802154-esp32s31, ieee802154-esp32s31-cli, lineage,
    provenance, scenario, scenario-cli}/
portable/{cooperate, crypto, diagnostics, events, memory, mmio, riscv, sort,
    sync, time, time-virtual, trace, wire}/
portable/embassy/{net, time}/
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
protocols/network/{core, port, runtime, wire}/
protocols/radio/{coex, port, regulatory}/
chips/esp32s31/{app, board, boot, bootstrap, compose, crypto, hal,
    instrumentation, knowledge, layout, pac, phy, wifi}/
chips/esp32s31/bluetooth/driver/
chips/esp32s31/ieee80211/driver/
chips/esp32s31/ieee802154/driver/
chips/esp32s31/radio/{client, system}/
chips/family/{analog, app, bluetooth-engine, boot, coex, efuse, executor, facts,
    ieee80211-policy, ieee802154-engine, ieee802154-mac-values, interrupt-table,
    modem-clock, phy, platform-port, power, radio, staged-bootstrap, staged-layout,
    staged-runtime, usb-serial-jtag, watchdog}/
oer/
examples/wifi-station/
hil/agent/
hil/images/{aes, wifi-station}/
hil/probes/{esp32s31-aes-timing, esp32s31-boot-smoke, esp32s31-facts,
    esp32s31-light-sleep, esp32s31-mcycle, esp32s31-peer-overdue, esp32s31-radio, esp32s31-radio-root,
    esp32s31-register, esp32s31-rx-chain}/
hil/scenarios/{aes, sdk-station, wifi-load, wifi-station}/
hil/services/{aes, fault, load, sdk-station, wifi-station}/
```

## Data and supporting files

- `chips/family/platform/` holds family and ISA fact layers;
  `chips/family/notes/` holds shared research notes.
- `chips/esp32s31/platform/` holds chip, module and board facts, partitions,
  ROM functions and stack budgets. `registers/` holds block types, layouts,
  the chip root, evidence and ownership policy. `notes/` holds research
  notes, and `vendor/` holds artifact pins and generated facts. `layout/linker/` holds the chip's linker fragments.
- `chips/family/executor/` contains the executor and its `bind!` macro,
  without a chip-specific feature or dependency. `chips/esp32s31/compose/`
  declares the `Esp32s31` executor binding over the HAL's platform port,
  alongside the chip's retained fault records and its platform, console and
  SAR/PWDET bindings.
- `chips/esp32c5/` contains `vendor/artifacts.toml` and `platform/` flash
  metadata and partitions for ESP-IDF peers; it contains no Rust package.
- `tools/lab/peers/` and `tools/vendor/references/` contain ESP-IDF projects, whose
  `firmware.toml` entries form the image catalog.
- `tools/lab/linux-net/` contains the network helper and pinned build inputs;
  `tools/lab/linux-bluetooth/` documents the Bluetooth helper.
- The root `Cargo.toml` and `Cargo.lock` define one Rust workspace.
  `.cargo/config.toml` defines the host command aliases.
  `target/` holds rebuildable output; generated evidence stores and caches
  are described by the tools that own them.

## Names

Every package is `oer-` and its whole path: `protocols/ieee80211/wire` is
`oer-protocols-ieee80211-wire`, `chips/esp32s31/hal` is
`oer-chips-esp32s31-hal`, `tools/image/cli` is `oer-tools-image-cli` and
`hil/probes/esp32s31-mcycle` is `oer-hil-probes-esp32s31-mcycle`. The
application entry (`oer/`) is `oer`. Rust imports replace hyphens with
underscores. A program called by name, such as the image linker or a
privileged helper, sets its binary name with `[[bin]] name`. `cargo fw`
names an image by its directory: an example or a probe by it alone, a HIL
image as `hil-<directory>` (`hil-wifi-station`).

## Layer roles

Each package under `chips/`, `protocols/hil/` and `hil/` (but `hil/probes/`) has one
role in `oer-tools-test-layers` that names its level
([architecture](architecture.md#layers)); a directory name such as `driver`
grants nothing. `chips/esp32s31/wifi` assembles the S31 station backend and portable runtime as the Wi-Fi facade.
A protocol leaf named `hil` takes its role from the list. A package under a protocol's
`contracts/` is a protocol contract, which a backend may use, except
`contracts/port`, which keeps the port's role; `testing/conformance` is a
backend conformance suite and `testing/model` a reference model, and no
product package depends on either; and
`protocols/bridges/*` are bridges, which stand only on the codecs they
join. Each `protocols/network/*` package has an explicit layer in
`oer-tools-test-layers`: the network codec stands on `oer-portable-wire` alone and the
network contract on the codec.

## Known deviations

- `protocols/ieee80211/runtime` has a dev-dependency on
  `chips/family/ieee80211-policy`.
- `chips/family/ieee80211-policy` has a dev-dependency on
  `chips/esp32s31/ieee80211/driver`.
- Normal dependencies outside their role:
  `chips/esp32s31/pac` renders the analog-I2C model types of
  `chips/family/analog`; `chips/esp32s31/hal` depends on
  `protocols/radio/coex`; `chips/esp32s31/ieee80211/driver` depends on
  `chips/family/staged-runtime` for its trace recorder;
  `chips/esp32s31/ieee802154/driver` depends on `portable/mmio` for
  `Placement`.
- HAL production files that wait or poll are listed with exact counts:
  modules of `bluetooth`, `boot`, `ieee80211` and `phy`, and
  `peripherals/entropy.rs`.
- A direct normal dependency on `embassy-sync` remains in
  `protocols/hil/agent`.

The layer tests enforce the normal and build dependency exceptions, the
source-file inclusions and the HAL files in their own deviation lists. The
family-to-chip normal/build deviation list is empty. Development
dependencies above are separate from those lists.
