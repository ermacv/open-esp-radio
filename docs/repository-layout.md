# Repository layout

Where code lives and what it may depend on.

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
`portable/hil/protocol` and `host/hil/run`, `portable/esp32s31/` (chip
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
| run a HIL scenario | `cargo hil run <scenario>` (builds through the image pipeline), or `--images <dir>` for images built before | `firmware/hil/*` and `host/hil/` speak `portable/hil/protocol` |
| add a HIL message | edit `portable/hil/protocol` | both sides build against one definition |
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then `cargo vendor-check` with the probe ELF | hook under `cfg(oer_probe)` in `chips/`; catalog in `portable/probe/` |
| run the 802.15.4 vendor stand | `cargo run -p oer-tool-vendor-ieee802154-esp32s31 -- compare <scenario>` | vendor driver and `portable/espressif/ieee802154-engine` meet at the low-level boundary; chip tables from `portable/esp32s31/` |
| check an image's stack | `cargo fw build <image>` | `host/image/checks` reads the ELF |
| add a register field | edit `registers/`, `cargo registers generate` | `chips/<chip>/pac` |

## Known deviations

The tree does not yet follow these rules in three places:

- `portable/ieee80211/mac` holds the Wi-Fi frame formats and protocol state
  together; chip backends and the radio system depend on it for formats
  alone (the radio system for the channel type only).
- `firmware/platform/<chip>/` holds chip data (`chip.toml`, partitions) and
  linker scripts that belong to `portable/<chip>/` and `chips/platform/`.
- Packages in `host/` still carry executables (the HIL fixture helpers, the
  fixture installer and its launchers, the image linker).
