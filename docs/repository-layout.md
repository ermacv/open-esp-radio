# Repository layout

The owner approved this layout on 2026-10-19; the migration map at the end is
the work list, removed once the tree matches it.

## Two kinds of package

- **Libraries** are compiled into something else. A library's root folder
  says where it can run.
- **Executables** link into one binary: an image for a chip (memory layout,
  linker script, chip target) or a program for the host.

Libraries are not split into "device" and "host" code: a sans-IO protocol
core runs in a firmware image, in a host emulator on virtual time, in a
future air simulator and inside host tools, unchanged. What separates
libraries is what they touch.

## Roots

```text
portable/   libraries that touch neither hardware nor an OS (no_std)
  base/ radio/ exec/ ieee80211/ bluetooth/ ieee802154/
  hil/          the HIL protocol, the agent core, conformance traces
  probe/        the probe catalog format
  espressif/    Espressif IP as algorithms and data (802.15.4 engine, coex, Wi-Fi policy)
  esp32s31/ esp32c5/   chip knowledge: tables, constants, layouts, pure functions
  platform/     memory layouts of the staged images
  adapters/     bindings to portable frameworks (Embassy)
chips/      libraries that access a chip's hardware (MMIO)
  espressif/    PACs of Espressif blocks
  esp32s31/ esp32c5/   PAC, HAL, PHY, drivers
  platform/     staged bootstrap and runtime, boards
  adapters/     esp-hal
host/       libraries that need an OS (std)
  os/ test/ analysis/ chip/ pins/ registers/ probe/ image/ lab/ hil/ vendor/
firmware/   images for a chip: apps/ bootstrap/ hil/ probes/ (and ESP-IDF peers/, vendor/)
tools/      programs for the host: fw/ hil/ stand/ registers/ blobray/ pins/ vendor/ …
registers/ knowledge/ docs/   data and documents
```

A domain may appear under several roots, split by what each part touches,
the way `defmt` (portable) and `defmt-decoder` (host) split one format:
`portable/hil/protocol` and `host/hil/runner`, `portable/esp32s31/` (chip
knowledge) and `chips/esp32s31/` (hardware access).

The roots follow embedded systems written in Rust: tock (`kernel/`,
`capsules/` portable; `arch/`, `chips/` hardware; `boards/` images; `tools/`
host) and hubris (`sys/`, `lib/` portable; `drv/`, `chips/` hardware; `app/`
images; `build/` host).

## Chip knowledge is not hardware access

A table, a constant, a layout, a register sequence described as data or a
pure function about a chip is **chip knowledge** and lives in
`portable/<chip>/` or `portable/espressif/`. Code that reads or writes the
chip's registers is **hardware access** and lives in `chips/`. A host tool
that needs a chip fact takes it from `portable/`; it never links a HAL. The
same split is `esp-metadata` beside `esp-hal`.

## A protocol: wire, port, cores, backends

Each radio protocol has three portable layers in `portable/<protocol>/`:

| Layer | Holds | Depends on |
|---|---|---|
| `wire` | the over-the-air formats: parsing and building frames and PDUs, information elements; no state, no time | `portable/base` |
| `port` | the contract with a backend: commands, events, resources, errors | `wire` |
| cores | the sans-IO state machines (station, access point, RSN, an LE connection, the 802.15.4 MAC) | `port`, `wire` |

A backend implements the port and depends on `port` and `wire` only, never
on a core: the chip backend in `chips/<chip>/`, the model backend in
`portable/<protocol>/` for host tests and simulation. A core never depends
on a backend. Structures both sides share belong to `port` or `wire`. Host
tools that read frames (monitors, capture analysis, fixtures, the air
model of a simulator) take `wire`. This is smoltcp's `wire` beside its
`iface` and `socket`.

Today the Wi-Fi MAC crate holds the formats and the state together, and the
chip backends and the radio system reach it for formats alone (the radio
system for the channel type only). Wi-Fi splits at W2 and W3, where the
transmit frames and the port's backend are built; Bluetooth LE and
IEEE 802.15.4 split in their blocks.

## Dependency rules

The layer test checks every normal dependency against its root. There are
no exceptions.

| From | May depend on |
|---|---|
| `portable/` | `portable/` |
| `chips/` | `portable/`, `chips/` |
| `host/` | `portable/`, `host/` |
| `firmware/` | `portable/`, `chips/` |
| `tools/` | `portable/`, `host/` |

- A crate in `portable/` is `no_std` and builds for every chip target in CI.
  A `no_std` helper that only host tools use may stay in `host/`.
- Build-time code (proc macros, `build.rs` helpers) runs on the host during a
  build; it lives in `host/` and is reached only as a proc-macro or build
  dependency.
- Inside a root, a domain depends on the domains below it; the order is
  listed once in the layer test. The order of `host/`, lowest first: `os`,
  `test`, `analysis`, `chip`, `pins`, `registers`, `probe`, `image`, `lab`,
  `hil`, `vendor`.
- Nothing optional crosses a rule: a portable crate does not reach esp-hal
  through a feature either. A crate that does belongs to `chips/` or splits.

## Names

A library's package name is `oer-` and its path below the root:
`portable/ieee80211/mac` is `oer-ieee80211-mac`, `chips/esp32s31/hal` is
`oer-esp32s31-hal`, `host/hil/run` is `oer-hil-run`. The layer test
refuses two libraries with the same name below different roots. An
executable keeps its root: `firmware/apps/w1` is `oer-firmware-apps-w1`,
`tools/fw` is `oer-tool-fw`. There is no `[lib] name`.

## One program, one folder

A host program is one package `tools/<tool>/`: `main.rs` and its own modules
(subcommands, argument parsing, output), as in `probe-rs-tools`. Code moves
into a library only when a second program or a library needs it.

## Artifacts, not builds

`cargo fw` is the one builder of images: apps, HIL agents, probes and,
through ESP-IDF, the vendor peers and references. It writes each image and a
manifest below `target/fw/`. Every other program takes artifacts: an ELF or
binary path, a manifest or a byte stream from a board. A command that builds
and runs is a thin wrapper over `cargo fw` and the program.

## Scenarios

| I want to | Command | Where |
|---|---|---|
| run an app on a board | `cargo fw run w1 --board s31-a` | `firmware/apps/w1`, `tools/fw` |
| run a HIL scenario | `cargo fw build …`, then `cargo hil run` with the manifest | `firmware/hil/*` and `host/hil/` speak `portable/hil/protocol` |
| add a HIL message | edit `portable/hil/protocol` | both sides build against one definition |
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then `cargo vendor-check` with the probe ELF | hook under `cfg(oer_probe)` in `chips/`; catalog in `portable/probe/` |
| run the 802.15.4 vendor stand | `cargo run -p oer-tool-vendor-ieee802154-esp32s31 -- compare <scenario>` | vendor driver and `portable/espressif/ieee802154-engine` meet at the low-level boundary; chip tables from `portable/esp32s31/` |
| check an image's stack | `cargo fw build <image>` | `host/image/checks` reads the ELF |
| add a register field | edit `registers/`, `cargo registers generate` | `chips/<chip>/pac` |

## Migration map

Each move is mechanical except two: the chip knowledge that host tools take
from the S31 HAL and PHY today (the TX power levels and the coexistence
priority tables, and the PHY calibration relation) moves to
`portable/esp32s31/`. Duplicates close inside their domains: one image
pipeline (`host/image/build`), one board path (`host/lab/board`), one chip
profile. ELF is read through `object` and no package parses it itself:
convenient reads through `host/analysis/elf`, bounded reads of untrusted
vendor inputs through `host/analysis/blobray-artifacts`.

| Today | Target | Level |
|---|---|---|
| `portable/adapters/embassy/net` | `portable/adapters/embassy/net` | portable |
| `portable/adapters/embassy/time` | `portable/adapters/embassy/time` | portable |
| `portable/base/deadline` | `portable/base/deadline` | portable |
| `portable/base/memory` | `portable/base/memory` | portable |
| `portable/base/network` | `portable/base/network` | portable |
| `portable/base/time` | `portable/base/time` | portable |
| `portable/base/time-virtual` | `portable/base/time-virtual` | portable |
| `portable/base/trace` | `portable/base/trace` | portable |
| `portable/bluetooth/hci` | `portable/bluetooth/hci` | portable |
| `portable/bluetooth/le/controller` | `portable/bluetooth/le/controller` | portable |
| `portable/bluetooth/le/ll` | `portable/bluetooth/le/ll` | portable |
| `portable/bluetooth/le/radio` | `portable/bluetooth/le/radio` | portable |
| `portable/esp32s31/phy-relation` | `portable/esp32s31/phy-relation` | portable |
| `portable/espressif/analog` | `portable/espressif/analog` | portable |
| `portable/espressif/coex` | `portable/espressif/coex` | portable |
| `portable/espressif/ieee80211-policy` | `portable/espressif/ieee80211-policy` | portable |
| `portable/espressif/ieee802154-engine` | `portable/espressif/ieee802154-engine` | portable |
| `portable/espressif/ieee802154-pac` | `portable/espressif/ieee802154-pac` | portable |
| `portable/espressif/modem-clock` | `portable/espressif/modem-clock` | portable |
| `portable/espressif/soc-deadline` | `portable/espressif/soc-deadline` | portable |
| `portable/exec/interrupt-table` | `portable/exec/interrupt-table` | portable |
| `portable/hil/agent` | `portable/hil/agent` | portable |
| `portable/hil/image-keys` | `portable/hil/protocol` | portable |
| `portable/hil/protocol` | `portable/hil/protocol` | portable |
| `portable/hil/trace` | `portable/hil/trace` | portable |
| `portable/hil/trace-header` | `portable/hil/trace` | portable |
| `portable/ieee80211/ap` | `portable/ieee80211/ap` | portable |
| `portable/ieee80211/arbiter` | `portable/ieee80211/arbiter` | portable |
| `portable/ieee80211/datapath` | `portable/ieee80211/datapath` | portable |
| `portable/ieee80211/lower-mac` | `portable/ieee80211/lower-mac` | portable |
| `portable/ieee80211/mac` | `portable/ieee80211/mac` | portable |
| `portable/ieee80211/rsn` | `portable/ieee80211/rsn` | portable |
| `portable/ieee80211/rsn-service` | `portable/ieee80211/rsn-service` | portable |
| `portable/ieee80211/runtime` | `portable/ieee80211/runtime` | portable |
| `portable/ieee80211/softmac` | `portable/ieee80211/softmac` | portable |
| `portable/ieee80211/sta` | `portable/ieee80211/sta` | portable |
| `portable/ieee80211/upper-mac` | `portable/ieee80211/upper-mac` | portable |
| `portable/ieee802154/mac` | `portable/ieee802154/mac` | portable |
| `portable/platform/esp32c5-layout` | `portable/platform/esp32c5-layout` | portable |
| `portable/platform/esp32s31-layout` | `portable/platform/esp32s31-layout` | portable |
| `portable/platform/staged-layout` | `portable/platform/staged-layout` | portable |
| `portable/radio/coex` | `portable/radio/coex` | portable |
| `portable/radio/permit-pool` | `portable/radio/permit-pool` | portable |
| `portable/radio/port` | `portable/radio/port` | portable |
| `chips/adapters/esp-hal/radio` | `chips/adapters/esp-hal/radio` | chip |
| `chips/adapters/esp-hal/soc` | `chips/adapters/esp-hal/soc` | portable |
| `chips/adapters/esp-hal/soc-esp32c5` | `chips/adapters/esp-hal/soc-esp32c5` | portable |
| `chips/esp32c5/hal` | `chips/esp32c5/hal` | chip |
| `chips/esp32c5/pac` | `chips/esp32c5/pac` | chip |
| `chips/esp32c5/pac-raw` | `chips/esp32c5/pac-raw` | chip |
| `chips/esp32s31/bluetooth/driver` | `chips/esp32s31/bluetooth/driver` | chip |
| `chips/esp32s31/bluetooth/memory` | `chips/esp32s31/bluetooth/memory` | chip |
| `chips/esp32s31/hal` | `chips/esp32s31/hal` | chip |
| `chips/esp32s31/ieee80211/driver` | `chips/esp32s31/ieee80211/driver` | chip |
| `chips/esp32s31/ieee80211/mac` | `chips/esp32s31/ieee80211/mac` | chip |
| `chips/esp32s31/pac` | `chips/esp32s31/pac` | chip |
| `chips/esp32s31/pac-raw` | `chips/esp32s31/pac-raw` | chip |
| `chips/esp32s31/phy` | `chips/esp32s31/phy` | chip |
| `chips/esp32s31/radio/client` | `chips/esp32s31/radio/client` | chip |
| `chips/esp32s31/radio/coex` | `chips/esp32s31/radio/coex` | chip |
| `chips/esp32s31/radio/system` | `chips/esp32s31/radio/system` | chip |
| `chips/espressif/ieee80211-pac` | `chips/espressif/ieee80211-pac` | chip |
| `chips/espressif/ieee80211-pac-raw` | `chips/espressif/ieee80211-pac-raw` | chip |
| `chips/exec/esp-hal-irq` | `chips/exec/esp-hal-irq` | portable |
| `chips/exec/executor` | `chips/exec/executor` | portable |
| `chips/platform/esp32c5-board` | `chips/platform/esp32c5-board` | chip |
| `chips/platform/esp32s31-board` | `chips/platform/esp32s31-board` | chip |
| `chips/platform/staged-bootstrap` | `chips/platform/staged-bootstrap` | chip |
| `chips/platform/staged-runtime` | `chips/platform/staged-runtime` | chip |
| `host/analysis/blobray-analysis` | `host/analysis/blobray-analysis` | host |
| `host/analysis/blobray-application` | `host/analysis/blobray-application` | host |
| `host/analysis/blobray-artifacts` | `host/analysis/blobray-artifacts` | host |
| `host/analysis/blobray-domain` | `host/analysis/blobray-domain` | host |
| `host/analysis/blobray-linker` | `host/analysis/blobray-linker` | host |
| `host/analysis/blobray-riscv` | `host/analysis/blobray-riscv` | host |
| `host/analysis/blobray-verification` | `host/analysis/blobray-verification` | host |
| `host/analysis/elf` | `host/analysis/elf` | host |
| `host/analysis/riscv-analysis` | `host/analysis/riscv-analysis` | host |
| `host/analysis/riscv-decode` | `host/analysis/riscv-decode` | portable |
| `host/analysis/riscv-lift` | `host/analysis/riscv-lift` | host |
| `host/analysis/riscv-model` | `host/analysis/riscv-model` | host |
| `host/analysis/riscv-program` | `host/analysis/riscv-program` | host |
| `host/analysis/riscv-stack` | `host/analysis/riscv-stack` | host |
| `host/hil/analysis` | `host/hil/analysis` | host |
| `host/hil/stats` | `host/hil/analysis` | host |
| `host/hil/family-phy` | `host/vendor/phy` | host |
| `host/hil/family-phy-esp32s31` | `host/vendor/phy-esp32s31` | host |
| `host/hil/family-system` | `host/hil/family-system` | host |
| `host/hil/fixture` | `host/hil/fixture` | host |
| `host/hil/link` | `host/hil/link` | host |
| `host/hil/image-class` | `host/hil/model` | host |
| `host/hil/model` | `host/hil/model` | host |
| `host/hil/scenario-catalog` | `host/hil/model` | host |
| `host/hil/schema` | `host/hil/model` | host |
| `host/hil/net-traffic` | `host/hil/net-traffic` | host |
| `host/hil/executor` | `host/hil/executor` | host |
| `host/hil/flash` | `host/hil/run` | host |
| `host/hil/image` | `host/hil/run` | host |
| `host/hil/lab` | `host/hil/lab` | host |
| `host/hil/observer` | `host/hil/observer` | host |
| `host/hil/runner` | `host/hil/run`, `tools/hil-runner` | host |
| `host/hil/run-bundle-format` | `host/hil/runs` | host |
| `host/hil/run-bundle` | `host/hil/runs` | host |
| `host/hil/scenario` | `host/hil/scenario` | host |
| `host/hil/workload` | `host/hil/scenario` | host |
| `host/image/firmware` | `tools/fw` | host |
| `host/image/build` | `host/image/build` | host |
| `host/image/bundle` | `host/image/build` | host |
| `host/image/encode` | `host/image/build` | host |
| `host/image/check-interrupts` | `host/image/checks` | host |
| `host/image/check-placement` | `host/image/checks` | host |
| `host/image/check-stack` | `host/image/checks` | host |
| `host/image/checks` | `host/image/checks` | host |
| `host/image/linker` | `host/image/checks` | host |
| `host/image/compare` | `host/image/compare` | host |
| `host/image/esp-idf` | `host/image/esp-idf` | host |
| `host/image/chip-profile` | `host/chip/profile` | host |
| `host/image/policy` | `host/chip/profile` | host |
| `host/image/toolchain` | `host/os/toolchain` | host |
| `host/lab/stand-arbiter` | `host/lab/arbiter` | host |
| `host/lab/stand-claims` | `host/lab/arbiter` | host |
| `host/lab/stand-journal` | `host/lab/arbiter` | host |
| `host/lab/stand-owners` | `host/lab/arbiter` | host |
| `host/lab/stand-resource-lock` | `host/lab/arbiter` | host |
| `host/lab/device-mac` | `host/lab/model` | host |
| `host/lab/device-lock` | `host/lab/board` | host |
| `host/lab/device-peer-line` | `host/lab/board` | host |
| `host/lab/devices` | `host/lab/board` | host |
| `host/lab/stand-board` | `host/lab/board` | host |
| `host/lab/stand-power` | `host/lab/board` | host |
| `host/lab/stand-fixture-install` | `host/lab/fixtures` | host |
| `host/lab/stand-fixtures` | `host/lab/router` | host |
| `host/lab/fixtures-bluetooth` | `host/hil/fixtures-bluetooth` | host |
| `host/lab/model` | `host/lab/model` | host |
| `host/lab/router` | `host/lab/router` | host |
| `host/lab/stand-hostapd` | `host/lab/router` | host |
| `host/lab/stand-ssh` | `host/lab/router` | host |
| `host/os/command-tree` | `host/os/command-tree` | host |
| `host/os/durable` | `host/os/durable` | host |
| `host/os/process` | `host/os/process` | host |
| `host/probe/codegen` | `host/probe/codegen` | host |
| `host/probe/macros` | `host/probe/macros` | build |
| `host/registers/bindings` | `host/registers/bindings` | host |
| `host/registers/contracts` | `host/registers/contracts` | host |
| `host/registers/model` | `host/registers/model` | host |
| `host/registers/review` | `host/registers/review` | host |
| `host/test/layers` | `host/test/layers` | host |
| `host/test/ui-cases` | `host/test/ui-cases` | host |
| `host/vendor/evidence-shard` | `host/vendor/evidence` | host |
| `host/vendor/evidence` | `host/vendor/evidence` | host |
| `host/vendor/ieee802154-esp32s31` | `host/vendor/ieee802154-esp32s31`, `tools/vendor-ieee802154-esp32s31` | host |
| `host/vendor/lineage` | `host/vendor/lineage`, `tools/vendor` | host |
| `host/vendor/phy-archive` | `host/vendor/phy`, `tools/vendor` | host |
| `host/vendor/phy-calibration-capture` | `host/vendor/phy` | host |
| `host/vendor/phy-esp32s31` | `host/vendor/phy-esp32s31` | host |
| `host/vendor/artifacts` | `host/pins` | host |
| `host/vendor/pins` | `host/pins` | host |
| `host/vendor/markers` | `host/vendor/provenance` | host |
| `host/vendor/provenance` | `host/vendor/provenance` | host |
| `host/vendor/scenario-engine` | `host/vendor/scenario` | host |
| `host/vendor/scenario-report` | `host/vendor/scenario-report` | host |
| `host/vendor/scenarios-esp32c5` | `host/vendor/scenarios-esp32c5` | host |
| `host/vendor/scenarios-esp32s31` | `host/vendor/scenarios-esp32s31` | host |
| `firmware/apps/boot-smoke` | (unchanged) | chip |
| `firmware/apps/radio-root` | (unchanged) | chip |
| `firmware/apps/w0` | (unchanged) | chip |
| `firmware/apps/w1` | (unchanged) | chip |
| `firmware/bootstrap/esp32c5` | `firmware/bootstrap/esp32c5` | chip |
| `firmware/bootstrap/esp32s31` | `firmware/bootstrap/esp32s31` | chip |
| `firmware/hil/esp32c5` | `firmware/hil/esp32c5` | chip |
| `firmware/hil/esp32s31` | `firmware/hil/esp32s31` | chip |
| `firmware/probes/esp32c5-radio` | `firmware/probes/esp32c5-radio` | chip |
| `firmware/probes/esp32c5-radio-elf` | `firmware/probes/esp32c5-radio` | chip |
| `firmware/probes/esp32c5-register` | `firmware/probes/esp32c5-register` | chip |
| `firmware/probes/esp32s31-boot-state` | `firmware/probes/esp32s31-boot-state` | chip |
| `firmware/probes/esp32s31-radio` | `firmware/probes/esp32s31-radio` | chip |
| `firmware/probes/esp32s31-radio-elf` | `firmware/probes/esp32s31-radio` | chip |
| `firmware/probes/esp32s31-register` | `firmware/probes/esp32s31-register` | chip |
| `firmware/probes/esp32s31-register-elf` | `firmware/probes/esp32s31-register` | chip |
| `tools/blobray` | `tools/blobray` | host |
| `tools/fw` | `tools/fw` | host |
| `tools/hil` | `tools/hil` | host |
| `host/hil/experiment` | `tools/hil` | host |
| `tools/registers` | `tools/registers` | host |
| `host/registers/generate` | `tools/registers` | host |
| `tools/stand` | `tools/stand` | host |
| `host/lab/stand-discover` | `tools/stand` | host |
| `host/lab/stand-doctor` | `tools/stand` | host |
| `tools/pins` | `tools/pins` | host |
| `tools/vendor-scenarios-esp32c5` | `tools/vendor-scenarios-esp32c5` | host |
| `tools/vendor-scenarios-esp32s31` | `tools/vendor-scenarios-esp32s31` | host |
| `tools/verification` | `tools/vendor` | host |
| `host/vendor/scenario-command` | `host/vendor/scenario-report` | host |
