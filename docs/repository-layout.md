# Repository layout

Where code and data live, and how packages are named.

## Two kinds of package

- **Libraries** are compiled into something else.
- **Executables** link into one binary: an image for a chip (memory layout,
  linker script, chip target) or a program for the developer's machine.
  They live only in `firmware/` and `tools/`; no library root has a binary
  target.

A sans-IO protocol core runs unchanged in a firmware image, in a host test
on virtual time and inside a host tool; libraries are not split into
"device" and "host" code.

## Roots

The root says what the code is about.

```text
protocols/  sans-IO protocols and their vocabularies (no_std)
  radio/        the radio port, coexistence vocabulary, regulatory domain, permit pool
  ieee80211/    Wi-Fi: MAC formats and state, lower and upper MAC, station,
                access point, RSN, arbiter, runtime, datapath
  bluetooth/    HCI and the LE link layer, controller, radio vocabulary, port and model
  ieee802154/   the IEEE 802.15.4 MAC
  network/      the network stack boundary
  hil/          the HIL protocol and its trace header
portable/   generic support no protocol or chip owns (no_std)
  time/ time-virtual/ memory/ sync/ trace/ wire/
  riscv/        architectural RISC-V constants and instructions
  embassy/      bindings to Embassy (time, net)
  hil-agent/    the agent core the HIL images run
chips/      everything about Espressif hardware
  espressif/    IP shared by Espressif chips: analog, modem
                clock, coexistence, the 802.15.4 engine, Wi-Fi policy, the
                staged boot (bootstrap, runtime, layout), interrupt table,
                executor, the chip facts model (facts/) and the
                family and ISA fact layers (platform/); registers/ and notes/
                shared by the chips
  esp32s31/ esp32c5/   (esp32c5 lies outside the Cargo workspace and CI)
                PAC, HAL, PHY, drivers, radio system, board, memory layout,
                chip knowledge, and the chip's data:
                platform/   chip, module and board fact layers, partitions, ROM
                            functions, stack budgets
                registers/  block types, layouts, chip root, evidence and ownership policy
                notes/      recovered facts and research notes
                vendor/     the vendor pins and generated facts
firmware/   images for a chip: apps/ bootstrap/ hil/ probes/, ESP-IDF peers/ and vendor/
tools/      everything that runs on the developer's machine, by domain:
  os/ test/ analysis/ chip/ pins/ regulatory/ registers/ probe/ image/ lab/ hil/ vendor/
docs/       cross-cutting documents
```

A domain of `tools/` holds its libraries and its program: `tools/hil/run`
is a library, `tools/hil/cli` is `cargo hil`; the HIL acceptance catalog is
`tools/hil/acceptance/<chip>/`.

## Dependencies

Dependency direction is not checked. It follows from the roots: a protocol
or a portable crate knows no chip and no tool; a chip crate builds on
protocols and portable support; images and tools sit on top, and nothing
depends on them except through build, proc-macro and test dependencies
(`tools/probe/macros`, `tools/test/ui-cases`).

Chip knowledge (tables, constants, pure functions about a chip) lives in
`chips/`, beside the hardware access it describes; a host tool that needs a
chip fact takes it from a crate that does not touch hardware
(`chips/esp32s31/knowledge`) and never links a HAL.

`chips/espressif/interrupt-table` owns the Espressif interrupt matrix,
CLIC encodings, vector ABI and staged-runtime symbols. Its package is
`oer-espressif-interrupt-table`; its consumers are chip crates and firmware.

## A protocol: wire, port, cores, backends

Each radio protocol has these layers in `protocols/<protocol>/`:

| Layer | Holds | Depends on |
|---|---|---|
| `wire` | the over-the-air formats: parsing and building frames and PDUs, information elements; no state, no time | portable support |
| `cipher` | frame protection transforms — CCMP, BIP; no state, no time | `wire` |
| `port` | the contract with a backend: commands, events, resources, errors | `wire` |
| cores | the sans-IO state machines (station, access point, RSN, an LE connection, the 802.15.4 MAC) | `port`, `wire`, `cipher` |
| `runtime` | the async loop over the port: awaits port events and deadlines, feeds the cores, executes their actions | `port`, `wire`, cores, portable time and signals |

A backend implements the port and depends on `port`, `wire` and `cipher` only,
never on a core: the chip backend in `chips/<chip>/`, the reference model in
its own host crate `protocols/<protocol>/model` for host tests and simulation.
A core never depends on a backend. Structures both sides share belong to `port`
or `wire`. Host tools that read frames (monitors, capture analysis, fixtures)
take `wire`. This is smoltcp's `wire` beside its `iface` and `socket`.

Wi-Fi upper-MAC and the family policy select each attempt's rate and protection,
including the legacy control-frame rate. The hardware backend receives those
decisions through the port, or a fixed-rate `ManagementTxPlan`, and owns EDCA
backoff and MSDU lifetime enforcement. Core algorithms and family tables stay
above it. PN allocation, BA originator state and reorder state belong to the
cores. The backend takes a supplied PN and reports decoded completion/RX values.

Only the runtime awaits, and it binds to no executor: it takes time from
`oer-time`'s `Timer` and interrupt signals from `portable/`, spawns nothing
and returns futures that the composition in `firmware/` spawns. It does not
depend on an executor, `embassy-time`, `embassy-net` or a HAL;
`embassy-sync` and `embassy-futures` are allowed as executor-independent
utilities. Embassy itself is bound only in `portable/embassy`, the chip's
executor and the firmware composition.

## Names

A library's package name is `oer-` and its path below its root:
`protocols/ieee80211/wire` is `oer-ieee80211-wire`, `chips/esp32s31/hal` is
`oer-esp32s31-hal`, `tools/hil/run` is `oer-hil-run`. An executable keeps
its root: `firmware/apps/w1` is `oer-firmware-apps-w1`, `tools/image/cli` is
`oer-tool-image-cli`. There is no `[lib] name`; code uses the package name
with underscores. A program called by name (the image linker, a privileged
helper) sets that name with `[[bin]] name`.

`cargo test -p oer-test-layers` checks the names, the unsafe boundary
([unsafe](unsafe.md)), the Wi-Fi chip backends' production dependencies and
the reviewed raw-waker inventory ([notifications](execution-wakes.md)).

## One workspace

The repository's root is one workspace with one `Cargo.lock`. Its
`default-members` are every package but the chip-only ones. Images build
with `--profile firmware` for the chip target through `cargo fw`; the
aliases in `.cargo/config.toml` run the programs with `--profile tools`.

## Artifacts, not builds

`cargo fw` is the one builder of images: apps, HIL agents, probes and,
through ESP-IDF, the vendor peers and references. It writes each image and a
manifest below `target/fw/`. Every other program takes artifacts: an ELF or
binary path, a manifest or a byte stream from a board. A command that builds
and runs is a thin wrapper over `cargo fw` and the program.

## Scenarios

| I want to | Command | Where |
|---|---|---|
| run an app on a board | `cargo fw run w1 --board s31-a` | `firmware/apps/w1`, `tools/image/cli` |
| run a HIL scenario | `cargo hil run <scenario>` (builds through the image pipeline), or `--images <dir>` for images built before | `firmware/hil/*` and `tools/hil/` speak `protocols/hil/protocol` |
| add a HIL message | edit `protocols/hil/protocol` | both sides build against one definition |
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then `cargo vendor-check` with the probe ELF | an entry over the public API in `firmware/probes/`; catalog code in `tools/probe/` |
| trace, profile or force a path in an image | declare `instrumentation = ["trace" \| "profile" \| "debug-control"]` under the image's `[package.metadata.open-radio]`, then `cargo fw build <image>` | code under `cfg(oer_trace)`, `cfg(oer_profile)` or `cfg(oer_debug_control)`; `tools/image/build/src/instrumentation.rs` |
| run the 802.15.4 vendor stand | `cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- compare <scenario>` | vendor driver and `chips/espressif/ieee802154-engine` meet at the low-level boundary |
| declare an image's task stacks proven | `task-stack = "proven"` under the image's `[package.metadata.open-radio]`, then `cargo fw build <image>` | `tools/image/build/src/task_stack.rs`; the check in `tools/image/checks/src/stack.rs` |
| check an image's stack | `cargo fw build <image>` | `tools/image/checks` reads the ELF |
| add a register field | edit `chips/<chip>/registers/`, `cargo registers generate --chip <chip>` | `chips/<chip>/pac` |

## Known deviations

The tree does not yet follow these rules in these places:

- `protocols/ieee80211/runtime` has a dev-dependency on
  `chips/espressif/ieee80211-policy`.
