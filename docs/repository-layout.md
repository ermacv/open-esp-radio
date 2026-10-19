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
  bluetooth/    HCI and the LE link layer, controller and radio port
  ieee802154/   the IEEE 802.15.4 MAC
  network/      the network stack boundary
  hil/          the HIL protocol and its trace header
portable/   generic support no protocol or chip owns (no_std)
  time/ time-virtual/ memory/ trace/ interrupt-table/
  embassy/      bindings to Embassy (time, net)
  hil-agent/    the agent core the HIL images run
chips/      everything about Espressif hardware
  espressif/    IP shared by Espressif chips: Wi-Fi MAC PAC, analog, modem
                clock, coexistence, the 802.15.4 engine, Wi-Fi policy, the
                staged boot (bootstrap, runtime, layout), executor and
                esp-hal bindings; registers/ and notes/ shared by the chips
  esp32s31/ esp32c5/
                PAC, HAL, PHY, drivers, radio system, board, memory layout,
                chip knowledge, and the chip's data:
                platform/   chip profile, partitions, ROM functions, stack budgets
                registers/  the reviewed register model and its publications
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

## A protocol: wire, port, cores, backends

Each radio protocol has three layers in `protocols/<protocol>/`:

| Layer | Holds | Depends on |
|---|---|---|
| `wire` | the over-the-air formats: parsing and building frames and PDUs, information elements; no state, no time | portable support |
| `port` | the contract with a backend: commands, events, resources, errors | `wire` |
| cores | the sans-IO state machines (station, access point, RSN, an LE connection, the 802.15.4 MAC) | `port`, `wire` |

A backend implements the port and depends on `port` and `wire` only, never
on a core: the chip backend in `chips/<chip>/`, the model backend in
`protocols/<protocol>/` for host tests and simulation. A core never depends
on a backend. Structures both sides share belong to `port` or `wire`. Host
tools that read frames (monitors, capture analysis, fixtures) take `wire`.
This is smoltcp's `wire` beside its `iface` and `socket`.

## Names

A library's package name is `oer-` and its path below its root:
`protocols/ieee80211/mac` is `oer-ieee80211-mac`, `chips/esp32s31/hal` is
`oer-esp32s31-hal`, `tools/hil/run` is `oer-hil-run`. An executable keeps
its root: `firmware/apps/w1` is `oer-firmware-apps-w1`, `tools/image/cli` is
`oer-tool-image-cli`. There is no `[lib] name`; code uses the package name
with underscores. A program called by name (the image linker, a privileged
helper) sets that name with `[[bin]] name`.

`cargo test -p oer-test-layers` checks the names and the unsafe boundary
([unsafe](unsafe.md)).

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
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then `cargo vendor-check` with the probe ELF | hook under `cfg(oer_probe)` in `chips/`; catalog code in `tools/probe/` |
| trace, profile or force a path in an image | declare `instrumentation = ["trace" \| "profile" \| "debug-control"]` under the image's `[package.metadata.open-radio]`, then `cargo fw build <image>` | code under `cfg(oer_trace)`, `cfg(oer_profile)` or `cfg(oer_debug_control)`; `tools/image/build/src/instrumentation.rs` |
| run the 802.15.4 vendor stand | `cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- compare <scenario>` | vendor driver and `chips/espressif/ieee802154-engine` meet at the low-level boundary |
| check an image's stack | `cargo fw build <image>` | `tools/image/checks` reads the ELF |
| add a register field | edit `chips/<chip>/registers/`, `cargo registers generate` | `chips/<chip>/pac` |

## Known deviations

The tree does not yet follow these rules in one place:

- `protocols/ieee80211/mac` holds the Wi-Fi frame formats and protocol state
  together; chip backends and the radio system depend on it for formats
  alone (the radio system for the channel type only).
