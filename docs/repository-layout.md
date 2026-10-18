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
  os/ test/ analysis/ registers/ image/ lab/ hil/ vendor/ probe/
firmware/   images for a chip: apps/ bootstrap/ hil/ probes/ (and ESP-IDF peers/, vendor/)
tools/      programs for the host: fw/ hil/ stand/ registers/ blobray/ vendor/
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
  listed once in the layer test.
- Nothing optional crosses a rule: a portable crate does not reach esp-hal
  through a feature either. A crate that does belongs to `chips/` or splits.

## Names

A library's package name is `oer-` and its path below the root:
`portable/ieee80211/mac` is `oer-ieee80211-mac`, `chips/esp32s31/hal` is
`oer-esp32s31-hal`, `host/hil/runner` is `oer-hil-runner`. The layer test
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
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then the vendor tool with the probe ELF | hook under `cfg(oer_probe)` in `chips/`; catalog in `portable/probe/` |
| run the 802.15.4 vendor stand | the vendor tool | vendor driver and `portable/espressif/ieee802154-engine` meet at the low-level boundary; chip tables from `portable/esp32s31/` |
| check an image's stack | `cargo fw build <image>` | `host/image/checks` reads the ELF |
| add a register field | edit `registers/`, `cargo registers generate` | `chips/<chip>/pac` |

## Migration map

Each move is mechanical except two: the chip knowledge that host tools take
from the S31 HAL and PHY today (the TX power levels and the coexistence
priority tables, and the PHY calibration relation) moves to
`portable/esp32s31/`. Duplicates close inside their domains: one image
pipeline (`host/image/build`), one board path (`host/lab/board`), one chip
profile, ELF through `host/analysis/elf`.

| Today | Target | Level |
|---|---|---|
| `crates/adapters/embassy/net` | `portable/adapters/embassy/net` | portable |
| `crates/adapters/embassy/time` | `portable/adapters/embassy/time` | portable |
| `crates/base/deadline` | `portable/base/deadline` | portable |
| `crates/base/memory` | `portable/base/memory` | portable |
| `crates/base/network` | `portable/base/network` | portable |
| `crates/base/time` | `portable/base/time` | portable |
| `crates/base/time-virtual` | `portable/base/time-virtual` | portable |
| `crates/base/trace` | `portable/base/trace` | portable |
| `crates/bluetooth/hci` | `portable/bluetooth/hci` | portable |
| `crates/bluetooth/le/controller` | `portable/bluetooth/le/controller` | portable |
| `crates/bluetooth/le/ll` | `portable/bluetooth/le/ll` | portable |
| `crates/bluetooth/le/radio` | `portable/bluetooth/le/radio` | portable |
| `tools/lib/analysis/phy-relation-esp32s31` | `portable/esp32s31/phy-relation` | portable |
| `crates/espressif/analog` | `portable/espressif/analog` | portable |
| `crates/espressif/coex` | `portable/espressif/coex` | portable |
| `crates/espressif/ieee80211-policy` | `portable/espressif/ieee80211-policy` | portable |
| `crates/espressif/ieee802154-engine` | `portable/espressif/ieee802154-engine` | portable |
| `crates/espressif/ieee802154-pac` | `portable/espressif/ieee802154-pac` | portable |
| `crates/espressif/modem-clock` | `portable/espressif/modem-clock` | portable |
| `crates/espressif/soc-deadline` | `portable/espressif/soc-deadline` | portable |
| `crates/exec/interrupt-table` | `portable/exec/interrupt-table` | portable |
| `firmware/hil/agent` | `portable/hil/agent` | portable |
| `tools/lib/data/hil-image-keys` | `portable/hil/protocol` | portable |
| `tools/lib/data/hil-protocol` | `portable/hil/protocol` | portable |
| `tools/lib/data/hil-trace` | `portable/hil/trace` | portable |
| `tools/lib/data/hil-trace-header` | `portable/hil/trace` | portable |
| `crates/ieee80211/ap` | `portable/ieee80211/ap` | portable |
| `crates/ieee80211/arbiter` | `portable/ieee80211/arbiter` | portable |
| `crates/ieee80211/datapath` | `portable/ieee80211/datapath` | portable |
| `crates/ieee80211/lower-mac` | `portable/ieee80211/lower-mac` | portable |
| `crates/ieee80211/mac` | `portable/ieee80211/mac` | portable |
| `crates/ieee80211/rsn` | `portable/ieee80211/rsn` | portable |
| `crates/ieee80211/rsn-service` | `portable/ieee80211/rsn-service` | portable |
| `crates/ieee80211/runtime` | `portable/ieee80211/runtime` | portable |
| `crates/ieee80211/softmac` | `portable/ieee80211/softmac` | portable |
| `crates/ieee80211/sta` | `portable/ieee80211/sta` | portable |
| `crates/ieee80211/upper-mac` | `portable/ieee80211/upper-mac` | portable |
| `crates/ieee802154/mac` | `portable/ieee802154/mac` | portable |
| `firmware/platform/esp32c5/layout` | `portable/platform/esp32c5-layout` | portable |
| `firmware/platform/esp32s31/layout` | `portable/platform/esp32s31-layout` | portable |
| `firmware/platform/espressif/staged-layout` | `portable/platform/staged-layout` | portable |
| `crates/radio/coex` | `portable/radio/coex` | portable |
| `crates/radio/permit-pool` | `portable/radio/permit-pool` | portable |
| `crates/radio/port` | `portable/radio/port` | portable |
| `crates/adapters/esp-hal/radio` | `chips/adapters/esp-hal/radio` | chip |
| `crates/adapters/esp-hal/soc` | `chips/adapters/esp-hal/soc` | portable |
| `crates/adapters/esp-hal/soc-esp32c5` | `chips/adapters/esp-hal/soc-esp32c5` | portable |
| `crates/esp32c5/hal` | `chips/esp32c5/hal` | chip |
| `crates/esp32c5/pac` | `chips/esp32c5/pac` | chip |
| `crates/esp32c5/pac-raw` | `chips/esp32c5/pac-raw` | chip |
| `crates/esp32s31/bluetooth/driver` | `chips/esp32s31/bluetooth/driver` | chip |
| `crates/esp32s31/bluetooth/memory` | `chips/esp32s31/bluetooth/memory` | chip |
| `crates/esp32s31/hal` | `chips/esp32s31/hal` | chip |
| `crates/esp32s31/ieee80211/driver` | `chips/esp32s31/ieee80211/driver` | chip |
| `crates/esp32s31/ieee80211/mac` | `chips/esp32s31/ieee80211/mac` | chip |
| `crates/esp32s31/pac` | `chips/esp32s31/pac` | chip |
| `crates/esp32s31/pac-raw` | `chips/esp32s31/pac-raw` | chip |
| `crates/esp32s31/phy` | `chips/esp32s31/phy` | chip |
| `crates/esp32s31/radio/client` | `chips/esp32s31/radio/client` | chip |
| `crates/esp32s31/radio/coex` | `chips/esp32s31/radio/coex` | chip |
| `crates/esp32s31/radio/system` | `chips/esp32s31/radio/system` | chip |
| `crates/espressif/ieee80211-pac` | `chips/espressif/ieee80211-pac` | chip |
| `crates/espressif/ieee80211-pac-raw` | `chips/espressif/ieee80211-pac-raw` | chip |
| `crates/exec/esp-hal-irq` | `chips/exec/esp-hal-irq` | portable |
| `crates/exec/executor` | `chips/exec/executor` | portable |
| `firmware/platform/esp32c5/board` | `chips/platform/esp32c5-board` | chip |
| `firmware/platform/esp32s31/board` | `chips/platform/esp32s31-board` | chip |
| `firmware/platform/espressif/staged-bootstrap` | `chips/platform/staged-bootstrap` | chip |
| `firmware/platform/espressif/staged-runtime` | `chips/platform/staged-runtime` | chip |
| `tools/lib/analysis/blobray-analysis` | `host/analysis/blobray-analysis` | host |
| `tools/lib/analysis/blobray-application` | `host/analysis/blobray-application` | host |
| `tools/lib/analysis/blobray-artifacts` | `host/analysis/blobray-artifacts` | host |
| `tools/lib/analysis/blobray-domain` | `host/analysis/blobray-domain` | host |
| `tools/lib/analysis/blobray-linker` | `host/analysis/blobray-linker` | host |
| `tools/lib/analysis/blobray-riscv` | `host/analysis/blobray-riscv` | host |
| `tools/lib/analysis/blobray-verification` | `host/analysis/blobray-verification` | host |
| `tools/lib/analysis/elf` | `host/analysis/elf` | host |
| `tools/lib/analysis/riscv-analysis` | `host/analysis/riscv-analysis` | host |
| `tools/lib/analysis/riscv-decode` | `host/analysis/riscv-decode` | portable |
| `tools/lib/analysis/riscv-lift` | `host/analysis/riscv-lift` | host |
| `tools/lib/analysis/riscv-model` | `host/analysis/riscv-model` | host |
| `tools/lib/analysis/riscv-program` | `host/analysis/riscv-program` | host |
| `tools/lib/analysis/riscv-stack` | `host/analysis/riscv-stack` | host |
| `tools/lib/service/hil-analysis` | `host/hil/analysis` | host |
| `tools/lib/service/stats` | `host/hil/analysis` | host |
| `tools/lib/service/hil-family-phy` | `host/hil/family-phy` | host |
| `tools/lib/service/hil-family-phy-esp32s31` | `host/hil/family-phy` | host |
| `tools/lib/service/hil-family-system` | `host/hil/family-system` | host |
| `tools/lib/service/hil-fixture` | `host/hil/fixture` | host |
| `tools/lib/service/hil-link` | `host/hil/link` | host |
| `tools/lib/data/hil-image-class` | `host/hil/model` | host |
| `tools/lib/data/hil-model` | `host/hil/model` | host |
| `tools/lib/data/hil-scenario-catalog` | `host/hil/model` | host |
| `tools/lib/data/hil-schema` | `host/hil/model` | host |
| `tools/lib/service/hil-net-traffic` | `host/hil/net-traffic` | host |
| `tools/lib/service/hil-executor` | `host/hil/runner` | host |
| `tools/lib/service/hil-flash` | `host/hil/runner` | host |
| `tools/lib/service/hil-image` | `host/hil/runner` | host |
| `tools/lib/service/hil-lab` | `host/hil/runner` | host |
| `tools/lib/service/hil-observer` | `host/hil/runner` | host |
| `tools/lib/service/hil-runner` | `host/hil/runner` | host |
| `tools/lib/data/hil-run-bundle-format` | `host/hil/runs` | host |
| `tools/lib/service/hil-run-bundle` | `host/hil/runs` | host |
| `tools/lib/service/hil-source-snapshot` | `host/hil/runs` | host |
| `tools/lib/service/hil-scenario` | `host/hil/scenario` | host |
| `tools/lib/service/hil-workload` | `host/hil/scenario` | host |
| `tools/lib/service/firmware` | `host/image/build` | host |
| `tools/lib/service/image` | `host/image/build` | host |
| `tools/lib/service/image-bundle` | `host/image/build` | host |
| `tools/lib/service/image-encode` | `host/image/build` | host |
| `tools/lib/service/image-check-interrupts` | `host/image/checks` | host |
| `tools/lib/service/image-check-placement` | `host/image/checks` | host |
| `tools/lib/service/image-check-stack` | `host/image/checks` | host |
| `tools/lib/service/image-checks` | `host/image/checks` | host |
| `tools/lib/service/image-linker` | `host/image/checks` | host |
| `tools/lib/service/image-compare` | `host/image/compare` | host |
| `tools/lib/service/esp-idf` | `host/image/esp-idf` | host |
| `tools/lib/data/chip-profile` | `host/image/profile` | host |
| `tools/lib/data/image-policy` | `host/image/profile` | host |
| `tools/lib/base/toolchain` | `host/image/toolchain` | host |
| `tools/lib/service/stand-arbiter` | `host/lab/arbiter` | host |
| `tools/lib/service/stand-claims` | `host/lab/arbiter` | host |
| `tools/lib/service/stand-journal` | `host/lab/arbiter` | host |
| `tools/lib/service/stand-owners` | `host/lab/arbiter` | host |
| `tools/lib/service/stand-resource-lock` | `host/lab/arbiter` | host |
| `tools/lib/data/device-mac` | `host/lab/board` | host |
| `tools/lib/service/device-lock` | `host/lab/board` | host |
| `tools/lib/service/device-peer-line` | `host/lab/board` | host |
| `tools/lib/service/devices` | `host/lab/board` | host |
| `tools/lib/service/stand-board` | `host/lab/board` | host |
| `tools/lib/service/stand-power` | `host/lab/board` | host |
| `tools/lib/service/stand-fixture-install` | `host/lab/fixtures` | host |
| `tools/lib/service/stand-fixtures` | `host/lab/fixtures` | host |
| `tools/lib/service/stand-fixture-bluetooth` | `host/lab/fixtures-bluetooth` | host |
| `tools/lib/data/stand-file` | `host/lab/model` | host |
| `tools/lib/service/lab` | `host/lab/router` | host |
| `tools/lib/service/stand-hostapd` | `host/lab/router` | host |
| `tools/lib/service/stand-ssh` | `host/lab/router` | host |
| `tools/lib/base/command-tree` | `host/os/command-tree` | host |
| `tools/lib/base/durable` | `host/os/durable` | host |
| `tools/lib/base/process` | `host/os/process` | host |
| `tools/lib/service/probe-codegen` | `host/probe/codegen` | host |
| `tools/lib/service/probe-macros` | `host/probe/macros` | build |
| `tools/lib/data/register-bindings` | `host/registers/bindings` | host |
| `tools/lib/data/register-contracts` | `host/registers/contracts` | host |
| `tools/lib/data/register-model` | `host/registers/model` | host |
| `tools/lib/data/register-review` | `host/registers/review` | host |
| `tools/lib/test/layers` | `host/test/layers` | host |
| `crates/base/ui-cases` | `host/test/ui-cases` | host |
| `tools/lib/data/vendor-evidence-shard` | `host/vendor/evidence` | host |
| `tools/lib/service/vendor-evidence` | `host/vendor/evidence` | host |
| `tools/lib/service/esp32s31-ieee802154-vendor-host` | `host/vendor/ieee802154-esp32s31` | host |
| `tools/lib/service/symbol-lineage` | `host/vendor/lineage` | host |
| `tools/lib/service/phy-archive` | `host/vendor/phy` | host |
| `tools/lib/service/phy-calibration-capture` | `host/vendor/phy` | host |
| `tools/lib/service/phy-vendor-calibration-esp32s31` | `host/vendor/phy-esp32s31` | host |
| `tools/lib/data/vendor-artifacts` | `host/vendor/pins` | host |
| `tools/lib/data/vendor-pins` | `host/vendor/pins` | host |
| `tools/lib/data/markers` | `host/vendor/provenance` | host |
| `tools/lib/service/vendor-provenance` | `host/vendor/provenance` | host |
| `tools/lib/service/vendor-scenario-engine` | `host/vendor/scenario` | host |
| `tools/lib/service/vendor-scenario-report` | `host/vendor/scenario` | host |
| `tools/lib/service/vendor-scenarios-esp32c5` | `host/vendor/scenarios-esp32c5` | host |
| `tools/lib/service/vendor-scenarios-esp32s31` | `host/vendor/scenarios-esp32s31` | host |
| `firmware/apps/boot-smoke` | (unchanged) | chip |
| `firmware/apps/radio-root` | (unchanged) | chip |
| `firmware/apps/w0` | (unchanged) | chip |
| `firmware/apps/w1` | (unchanged) | chip |
| `firmware/platform/esp32c5/bootstrap` | `firmware/bootstrap/esp32c5` | chip |
| `firmware/platform/esp32s31/bootstrap` | `firmware/bootstrap/esp32s31` | chip |
| `firmware/hil/esp32c5-agent` | `firmware/hil/esp32c5` | chip |
| `firmware/hil/esp32s31-agent` | `firmware/hil/esp32s31` | chip |
| `firmware/verification/esp32c5/probe-radio` | `firmware/probes/esp32c5-radio` | chip |
| `firmware/verification/esp32c5/probe-radio-elf` | `firmware/probes/esp32c5-radio` | chip |
| `firmware/verification/esp32c5/register-probe` | `firmware/probes/esp32c5-register` | chip |
| `firmware/verification/esp32s31/boot-state-probe` | `firmware/probes/esp32s31-boot-state` | chip |
| `firmware/verification/esp32s31/probe-radio` | `firmware/probes/esp32s31-radio` | chip |
| `firmware/verification/esp32s31/probe-radio-elf` | `firmware/probes/esp32s31-radio` | chip |
| `firmware/verification/esp32s31/probe-register` | `firmware/probes/esp32s31-register` | chip |
| `firmware/verification/esp32s31/probe-register-elf` | `firmware/probes/esp32s31-register` | chip |
| `tools/cli/blobray` | `tools/blobray` | host |
| `tools/cli/fw` | `tools/fw` | host |
| `tools/cli/hil` | `tools/hil` | host |
| `tools/lib/service/hil-experiment` | `tools/hil` | host |
| `tools/cli/registers` | `tools/registers` | host |
| `tools/lib/service/registers` | `tools/registers` | host |
| `tools/cli/stand` | `tools/stand` | host |
| `tools/lib/service/stand-discover` | `tools/stand` | host |
| `tools/lib/service/stand-doctor` | `tools/stand` | host |
| `tools/cli/pins` | `tools/vendor` | host |
| `tools/cli/vendor-scenarios-esp32c5` | `tools/vendor` | host |
| `tools/cli/vendor-scenarios-esp32s31` | `tools/vendor` | host |
| `tools/cli/verification` | `tools/vendor` | host |
| `tools/lib/service/vendor-scenario-command` | `tools/vendor` | host |
