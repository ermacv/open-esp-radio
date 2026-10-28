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
  radio/        the radio port with its FIFO permit pool, coexistence vocabulary,
                regulatory domain
  ieee80211/    Wi-Fi: MAC formats and state, lower and upper MAC, station,
                access point, RSN, arbiter, ESP-NOW, runtime
  bluetooth/    HCI and LE wire formats, link layer, controller, port, runtime and model
  ieee802154/   IEEE 802.15.4 wire codecs, backend port, MAC core and host model
  network/      the network stack boundary
  hil/          the HIL protocol, with the conformance trace header
portable/   generic support no protocol or chip owns (no_std)
  time/ time-virtual/ memory/ sync/ trace/ wire/
  crypto/       owned operation contracts (key unwrap) and mode algorithms
                (RFC 3394) over a synchronous block primitive, with conformance
  riscv/        architectural RISC-V constants and instructions
  embassy/      bindings to Embassy (time; net, with the radio TX datapath
                contracts)
  hil-agent/    the agent core the HIL images run
chips/      everything about Espressif hardware
  espressif/    IP shared by Espressif chips: analog, modem
                clock, coexistence, the 802.15.4 engine, Wi-Fi policy, the
                staged boot (bootstrap, runtime, layout), interrupt table,
                executor, the platform contracts a chip HAL implements
                (platform-port/), the chip facts model (facts/) and the
                family and ISA fact layers (platform/); registers/ and notes/
                shared by the chips
  esp32s31/     PAC, HAL, PHY, drivers, radio system, board, memory layout,
                chip knowledge, and the chip's data:
                platform/   chip, module and board fact layers, partitions, ROM
                            functions, stack budgets
                registers/  block types, layouts, chip root, evidence and ownership policy
                notes/      recovered facts and research notes
                vendor/     the vendor pins and generated facts
  esp32c5/      only the data of the ESP-IDF peers: vendor/artifacts.toml
                (the pins) and platform/ (flash.toml, the flash map of the
                peer board, and the partition table); no Rust code
firmware/   images for a chip: apps/ bootstrap/ hil/ probes/, ESP-IDF peers/ and vendor/
tools/      everything that runs on the developer's machine, by domain:
  os/ test/ analysis/ chip/ pins/ regulatory/ registers/ probe/ image/ lab/ hil/ vendor/
docs/       cross-cutting documents
```

A domain of `tools/` holds its libraries and its program: `tools/hil/run`
is a library, `tools/hil/cli` is `cargo hil`; the HIL acceptance catalog is
`tools/hil/acceptance/<chip>/`.

## Dependencies

Dependency direction follows from the roots: a protocol or a portable crate
knows no chip and no tool; chip backends build on protocol wire and port
contracts and portable support; images and tools sit on top. Nothing depends
on images or tools except through build, proc-macro and test dependencies
(`tools/probe/macros`, `tools/test/ui-cases`): no package takes a normal
dependency on a package of `firmware/` or on a program of `tools/` (a package
with a binary target). `chips/espressif` is the family below the chips and
depends on no `chips/<chip>` package, through a normal or a build dependency:
a chip's HAL implements the family's platform contracts
(`chips/espressif/platform-port`) on its own types and selects its chip
through the port's chip feature. `portable` depends on no protocol and no
chip; only `portable/embassy` adapts protocol boundaries. The layer tests check
normal transitive dependencies, including optional and target-specific
declarations: backends and reference models do not reach protocol cores or
runtimes.

Chip knowledge (tables, constants, pure functions about a chip) lives in
`chips/`, beside the hardware access it describes; a host tool that needs a
chip fact takes it from a crate that does not touch hardware
(`chips/esp32s31/knowledge`) and never links a HAL.

`chips/esp32s31/ieee802154/driver` binds the family engine's LL contract to
HAL transactions. It is a separate crate so HAL and chip knowledge have no
normal dependency on that engine or the driver. Shared frame storage and
opaque address tables live in `chips/espressif/ieee802154-mac-values`; HAL
alone allocates and projects the placed storage.

`chips/espressif/interrupt-table` owns the Espressif interrupt matrix,
vector ABI and staged-runtime symbols. `portable/riscv::clic` owns architectural
CLIC line values; chip facts select geometry and MNLBITS. `portable/memory::signal`
owns the portable signal cells, atomic facts and intrusive lists. Its package is
`oer-espressif-interrupt-table`; its consumers are chip crates and firmware.

## A protocol: wire, port, cores, backends

Each radio protocol has these layers in `protocols/<protocol>/`:

| Layer | Holds | Depends on |
|---|---|---|
| `wire` | the over-the-air formats: parsing and building frames and PDUs, information elements; no state, no time | portable support |
| `cipher` | frame protection transforms — CCMP, BIP — and the protocol's software block primitive under `oer-crypto`'s mode algorithms; no state, no time | `wire`, `oer-crypto` |
| `port` | the contract with a backend: commands, events, resources, errors | `wire` |
| cores | the sans-IO state machines (station, access point, RSN, an LE connection, the 802.15.4 MAC) | `port`, `wire`, `cipher` |
| `runtime` | the async loop over the port: awaits port events and deadlines, feeds the cores, executes their actions; parses and builds no frame | `port`, cores, portable time, signals and `oer-crypto` contracts |

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
above it. `chips/espressif/ieee80211-policy` is core-side family policy,
not a hardware backend. PN allocation, BA originator state and reorder state
belong to the cores. The backend takes a supplied PN and reports decoded completion/RX values.

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

`cargo test -p oer-test-layers` checks every package's name against its path,
the edges of Dependencies, the unsafe boundary
([unsafe](unsafe.md)), protocol core/runtime isolation from backends and
models through normal dependencies, the IEEE 802.11 layer table (the cipher,
port, model and runtime dependencies) and its cores that never wait, and the
reviewed raw-waker inventory ([notifications](execution-wakes.md)).

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
- `firmware/hil/esp32s31` -> `firmware/apps/w3` (removed by the IEEE 802.11
  facade, #844).
- `chips/espressif/executor` and `chips/espressif/staged-runtime` ->
  `chips/esp32s31/hal` (optional, feature `esp32s31`; removed by the staged
  runtime over the platform port and the executor over a chip binding, #848,
  B1).
- `portable/hil-agent` -> `protocols/hil/protocol` (removed by the move of
  hil-agent to `protocols/hil/agent`, #848, B6).

The layer test lists each of these edges; an edge that is gone must leave the
list, and the list gains no entry.
