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
| `hil/` | Checks on hardware: images on the entry (`images/`), probes below it (`probes/`), test-only service vocabularies (`services/`), the HIL agent that probes and images mount (`agent/`) and one host program per scenario (`scenarios/`). A probe differs from a scenario only in what it is allowed to use. A probe may use an API of any level, `unsafe` and any other means it needs; a scenario uses only the public API. The work between the DUT and the host is done through the HIL protocol and the agent. |
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
portable/{cooperate, crypto, diagnostics, events, memory, mmio, riscv,
    sync, time, time-virtual, trace, wire}/
portable/embassy/{net, time}/
protocols/bluetooth/hci/
protocols/bluetooth/le/{cipher, controller, ll, model, port, runtime, wire}/
protocols/bridges/ieee80211-ethernet/
protocols/hil/{agent, protocol}/
protocols/ieee802/wire/
protocols/ieee80211/{runtime, wire}/
protocols/ieee80211/contracts/{port, security}/
protocols/ieee80211/core/{arbiter, upper-mac}/
protocols/ieee80211/core/roles/{ap, sta}/
protocols/ieee80211/core/security/{cipher, rsn}/
protocols/ieee80211/testing/{conformance, model}/
protocols/ieee802154/{mac, model, port, wire}/
protocols/network/{core, port, runtime, wire}/
protocols/network/testing/model/
protocols/radio/{coex, port, regulatory}/
chips/esp32s31/{app, hal, knowledge, pac}/
chips/esp32s31/drivers/aes/
chips/esp32s31/platform/{board, bootstrap, layout, runtime}/
chips/esp32s31/radio/phy/
chips/family/{app, boot, facts}/
chips/family/drivers/{efuse, pmu, regi2c, system-timer, usb-serial-jtag,
    watchdog}/
chips/family/registers/analog-i2c/
chips/family/platform/{bootstrap, executor, interrupt-table, layout, port,
    runtime}/
chips/family/radio/{coex, modem-clock, phy, power}/
chips/family/radio/bluetooth/ports/
chips/family/radio/ieee80211/policy/
chips/family/radio/ieee802154/{engine, values}/
oer/
hil/agent/
hil/images/aes/
hil/probes/{esp32s31-boot-smoke, esp32s31-light-sleep, esp32s31-mcycle, esp32s31-peer-overdue, esp32s31-radio, esp32s31-radio-root,
    esp32s31-register, esp32s31-rx-ring, esp32s31-timer-facts}/
hil/scenarios/{aes, radio-pair, timer-facts}/
hil/services/{aes, radio-pair, timer-facts}/
```

The workspace's `exclude` names the packages that stay as sources and are
not built. Kept as sources until their radio returns:
- the Bluetooth LE and IEEE 802.15.4 backends
  (`chips/esp32s31/radio/{bluetooth, ieee802154}/backend/`);
- their protocols (`protocols/bluetooth/le/{cipher, controller, ll, model,
  port, runtime, wire}/`, `protocols/ieee802154/{mac, model, port, wire}/`);
- their family crates (`chips/family/radio/bluetooth/ports/`,
  `chips/family/radio/ieee802154/{engine, values}/`);
- the IEEE 802.15.4 host stand (`tools/vendor/ieee802154-esp32s31{,-cli}/`).

Kept too, until a plan takes it up: ESP-NOW
(`protocols/ieee80211/core/extensions/esp-now/`).

`protocols/bluetooth/hci/` stays built: the stand's Linux fixture helpers
(`tools/lab/fixture-helpers`) depend on it, the Wi-Fi fixture's programs
among them. Inside built packages, files stay as sources outside their
crate's module tree:
- the HAL's `radio/{bluetooth, ieee802154}` modules, with their
  compile-fail cases (`ui/` beside each);
- the PHY crate's `bluetooth_client` and `ieee802154_client`;
- the family application's observation counters
  (`chips/family/app/src/observe{.rs,/}`), whose one reader is
  `hil/images/wifi-station`, and the S31 application's `observed.rs`,
  `wifi.rs` and `wifi/`.
- the access point's SAE responder, until the access point's role serves
  SAE: `rsn`'s `authenticator/sae{.rs,/}`, the validity of a station's
  Commit and the anti-clogging tokens (`sae/access_point.rs`, with their
  compile-fail cases in `sae/access_point_ui/`), and the wire's received
  Commit (`sae/access_point.rs`);
- what no step of the current work reads: the wire's ESP-NOW formats
  (`extensions/espressif{.rs,/}`), NDP Announcement (`ndpa{.rs,/}`) and
  Trigger frames with HE control (`trigger{.rs,/}`), and the upper MAC's
  monitor channel sequence and policy (`monitor/channels.rs`).

Awaiting deletion at their successor:
- the old Wi-Fi path, its backend and station
  (`chips/esp32s31/radio/ieee80211/{backend, station}/`);
- the radio system and its client contract
  (`chips/esp32s31/radio/{system, client}/`);
- what stands on it: `examples/wifi-station/`,
  `hil/images/{wifi-station, wifi-access-point}/`,
  `hil/scenarios/{wifi-load, wifi-station}/`, `hil/services/{load, wifi-station}/`;
- what served only those images: `hil/services/{fault, sdk-station,
  wifi-access-point}/` and `hil/scenarios/{sdk-station, wifi-access-point}/`;
- the old backend's retained trace recorder (`portable/trace-recorder/`).

The station's acceptance suite
(`protocols/network/testing/model/station-suite/`) is kept beside its package
and returns to its `tests/` with the station.

## Data and supporting files

- `chips/family/facts/layers/` holds family and ISA fact layers.
- `chips/esp32s31/facts/` holds chip, module and board facts, partitions,
  ROM functions and stack budgets. `registers/` holds block types, layouts,
  the chip root, evidence and ownership policy, and `vendor/` holds artifact
  pins and generated facts. `platform/layout/linker/` holds the chip's linker fragments.
- `chips/family/platform/executor/` contains the executor and its `bind!` macro,
  without a chip-specific feature or dependency. `chips/esp32s31/platform/runtime/`
  declares the `Esp32s31` executor binding over the HAL's platform port,
  alongside the chip's retained fault records and its platform, console and
  SAR/PWDET bindings.
- `chips/esp32c5/` contains `vendor/artifacts.toml` and `facts/` flash
  metadata and partitions for ESP-IDF peers; it contains no Rust package.
- `tools/lab/peers/` and `tools/vendor/references/` contain ESP-IDF projects, whose
  `firmware.toml` entries form the image catalog.
- `tools/lab/linux-net/` contains the network helper and pinned build inputs;
  `tools/lab/linux-bluetooth/` documents the Bluetooth helper.
- The root `Cargo.toml` and `Cargo.lock` define one Rust workspace.
  `.cargo/config.toml` defines the host command aliases.
  `target/` holds rebuildable output; generated evidence stores and caches
  are described by the tools that own them.

## Chip crates

The family and each chip group first by subject, and a subject holds fixed
layers. A chip may hold a directory the family lacks: a family directory exists
only for code proved neutral.

| Word | Meaning |
| --- | --- |
| `hal` | Owners and hardware atoms over the PAC; one crate per chip |
| `drivers/`, a `driver` | Sequences and state over HAL atoms or a register port |
| `backend` | A chip's implementation of a protocol port |
| `engine` | Neutral logic of a protocol over its ports |
| `platform/` | Startup and execution |

`chips/<chip>/hal/src/` groups its modules in `system`, `power`, `peripherals`
and `radio`; `peripherals` there holds a block's atoms, not its driver.
`chips/<chip>/radio/` holds the crates above the HAL's radio atoms: the PHY
driver, and (as sources not built) the radio system, its client, and under
each protocol its backend and facade.

## Names

Every package is `oer-` and its whole path: `protocols/ieee80211/wire` is
`oer-protocols-ieee80211-wire`, `chips/esp32s31/hal` is
`oer-chips-esp32s31-hal`, `tools/image/cli` is `oer-tools-image-cli` and
`hil/probes/esp32s31-mcycle` is `oer-hil-probes-esp32s31-mcycle`. The
application entry (`oer/`) is `oer`. Rust imports replace hyphens with
underscores. A program called by name, such as the image linker or a
privileged helper, sets its binary name with `[[bin]] name`. `cargo fw`
names an image by its directory: an example or a probe by it alone, a HIL
image as `hil-<directory>` (`hil-aes`).

## Layer roles

Each package under `chips/`, `protocols/hil/` and `hil/` (but `hil/probes/`) has one
role in `oer-tools-test-layers` that names its level
([architecture](architecture.md#layers)); a directory name grants nothing.
A protocol leaf named `hil` takes its role from the list. A package under a protocol's
`contracts/` is a protocol contract, which a backend may use, except
`contracts/port`, which keeps the port's role; `testing/conformance` is a
backend conformance suite and `testing/model` a reference model, and no
product package depends on either; and
`protocols/bridges/*` are bridges, which stand only on the codecs they
join. Each `protocols/network/*` package has an explicit layer in
`oer-tools-test-layers`: the network codec stands on `oer-portable-wire` and the
IEEE 802 wire (`protocols/ieee802/wire`) alone and the network contract on the
two. The IEEE 802 wire holds the addresses and the EtherTypes both the IEEE 802.11 and the
network codecs carry and stands on `oer-portable-wire` alone.

## Known deviations

- `protocols/ieee80211/runtime` and `protocols/network/testing/model` have a
  dev-dependency on `chips/family/radio/ieee80211/policy`.
- A normal dependency outside its role: `chips/esp32s31/hal` depends on
  `protocols/radio/coex`.
- HAL production files that wait or poll are listed with exact counts:
  modules of `radio::bluetooth`, `system::boot`, `radio::ieee80211` and
  `radio::phy`.
- A direct normal dependency on `embassy-sync` remains in
  `protocols/hil/agent`.

The layer tests enforce the normal and build dependency exceptions, the
source-file inclusions and the HAL files in their own deviation lists. The
family-to-chip normal/build deviation list is empty. Development
dependencies above are separate from those lists.
