# Layer tests

`oer-test-layers` checks the structure rules of
[architecture](../../../docs/architecture.md) and
[repository layout](../../../docs/repository-layout.md) from the workspace
manifests and sources: package names, the unsafe boundary, the sans-IO lint
catalogue, layer roles and their dependencies, protocol isolation, and the
HAL's atoms. Every rule has synthetic cases beside it. Dependency rules read
normal dependencies, including optional, target-specific, aliased and
inherited entries; development dependencies are outside them, and build
dependencies count only where a rule says so.

## Roles

`tests/layers.rs` lists the role of every package under `chips/`,
`protocols/hil/` and `firmware/hil/` and of any `hil` leaf (`ROLES`); a
listed role whose package does not exist yet waits in `AWAITED_ROLES`. Other
packages follow their root: protocol leaves (`wire`, `cipher`, `port`,
`model`, `runtime`, otherwise a core; `instrumentation` is an
`ObservationContract`), `protocols/{radio,network}` shared vocabulary,
`portable/{mmio,riscv}` hardware access, `portable/embassy/*` adapters,
other `portable/` support, firmware images and tools.

| Role | Packages | Direct normal dependencies allowed |
| --- | --- | --- |
| `Facts` | `espressif/{facts,efuse,ieee802154-mac-values}`, `esp32s31/knowledge` | facts, portable, shared vocabulary, wire |
| `Pac` | `esp32s31/pac` | facts, portable, hardware access |
| `Hal` | `esp32s31/hal` | PAC, facts, platform contracts, family hardware drivers (their register ports), portable, hardware access |
| `HardwareDriver` | `espressif/{analog,boot,coex,ieee802154-engine,modem-clock,usb-serial-jtag,watchdog}`, `esp32s31/{boot,phy}` | HAL, hardware drivers, platform contracts, facts, observation contracts, portable, shared vocabulary, wire, port, cipher |
| `ChipAdapter` | `esp32s31/{bluetooth/driver,ieee80211/driver,ieee802154/driver}` | as a hardware driver, and chip adapters and service clients |
| `CoreSidePolicy` | `espressif/ieee80211-policy` | wire, port, cipher, cores, facts, observation contracts, portable, shared vocabulary |
| `ServiceClient` | `esp32s31/radio/client` | as a hardware driver |
| `Service` | `esp32s31/radio/system` | as a hardware driver, and service clients |
| `PlatformContract` | `espressif/{interrupt-table,platform-port,staged-layout}` | platform contracts, facts, portable, hardware access |
| `Platform` | `espressif/{executor,staged-bootstrap,staged-runtime}`, `esp32s31/{board,layout}` | platform, platform contracts, family hardware drivers, facts, shared vocabulary, portable, hardware access |
| `Composition` | `esp32s31/compose` | every chip role but PAC, composition and facade, every protocol role, portable, Embassy adapters, hardware access; no HIL |
| `Facade` | `esp32s31/wifi` | as a composition, and the composition |
| `ObservationContract` | `protocols/*/instrumentation`, `chips/esp32s31/instrumentation` | observation contracts, facts, shared vocabulary, portable, wire, port |
| `HilProtocol` | `protocols/hil/protocol` | portable |
| `HilAgent` | `protocols/hil/agent` | HIL protocol, portable |
| `HilBinding` | Wi-Fi, network, Bluetooth, conformance, PHY probe and radio-health dictionaries | HIL protocol and agent, facades, observation contracts, shared vocabulary, wire, port, runtime, portable without `portable/time{,-virtual}` |
| `HilImage` | `firmware/hil/*` | composition, facades, observation contracts, HIL bindings, agent and protocol, platform, platform contracts, portable, Embassy adapters |

Roles outside this table keep their own rules. The backend rule holds for
facts, PAC, HAL, drivers, adapters, service and client, platform, platform
contracts, observation contracts and protocol models: no normal path, direct
or transitive, reaches a protocol core, runtime or core-side policy. Only
`Composition` and `Facade` assemble runtimes with backends. The radio
service reaches no chip adapter by any path. The host HIL families
(`tools/hil/family-*`), including the PHY comparison family, reach
no HAL or PAC.

## Deviation lists

An edge, inclusion or HAL file that the tree still has against a rule is
listed with the change that removes it. A new entry and an entry whose edge
is gone both fail, so every list only shrinks:

- `OUTSIDE_THE_ROLES`: direct edges against the role table;
- `NO_EDGE_ON_AN_IMAGE_OR_PROGRAM`, `FAMILY_BELOW_THE_CHIPS`,
  `PORTABLE_BELOW_THE_PROTOCOLS`: edges onto images and programs, from the
  family onto a chip, from portable support onto a protocol or chip;
- `FIRMWARE_SOURCES_INCLUDED`: `#[path]` inclusions of `firmware/` sources
  from any other root;
- `HAL_SEQUENCES`: HAL files that wait.

- `EXTERNAL_ASYNC_CRATES`: direct normal `embassy-sync` edges in cooperate,
  the network adapter, the HIL agent and the three HIL images.

## External async crates

`external_embassy_crates_follow_the_role_table` checks direct normal edges
by package name, resolving `package =` and workspace inheritance and
including optional and target-specific entries. Development and build edges
are outside this rule; it does not inspect third-party dependency closures.

| Package | Allowed home or role |
| --- | --- |
| `embassy-futures` | All except wire, cipher, core, core-side policy, facts, PAC and HAL |
| `embassy-sync` | Development only, with the exact normal-edge deviations above |
| `embassy-time` | `portable/embassy/time`, `firmware/hil/esp32s31` (time ABI check) |
| `embassy-time-driver` | `chips/espressif/executor` |
| `embassy-net`, `xarxa`, `xarxa-driver` | `portable/embassy/net`, `firmware/*` |
| `trouble-host` | `portable/embassy/trouble`, `firmware/*` |
| `bt-hci` | `protocols/bluetooth/hci`, `portable/embassy/trouble`, `tools/*` |

`protocols_never_use_the_bt_hci_async_controller` scans Rust tokens in all
protocol sources, including tests, for `bt_hci::controller` paths and grouped
imports. Comments and strings do not count. This is a lexical check, not Rust
name resolution.

## Terminal-site registry

[`terminal-sites.toml`](terminal-sites.toml) records source sites with their
fault classes and reset owner. `tests/terminal_sites.rs` compares exact sets:
an added site, a missing record, a deleted site and a stale record all fail.
Negative fixtures change calls, remove files, add retained variants, reject
duplicate keys and exercise callback references, aliases and macro bodies.
A regression fixture shifts a site's line and column without changing its key.
The registry is an inventory, not permission to add a panic or reset path.

Each `[sites]` key is `path:item:kind:source-name:ordinal`. `item` is the
enclosing AST path from `crate`, including out-of-line and inline modules,
named items and methods. Inherent implementations use `impl Type`; trait
implementations use `impl Trait for Type`, with type/trait tokens normalized
independently of source whitespace. Named macro definitions use their name;
unnamed macro invocations belong to the enclosing module or named item.
Literal `include!` files inherit the including module's path.
The 1-based `ordinal` counts only sites with the same item, kind and source
name, in AST/token visitation order. Every key includes it, including unique
sites, so appending a repeat preserves existing keys. Inserting or reordering
identical sites requires reviewing their classifications.

For example, `firmware/apps/boot-smoke/src/main.rs:crate::run:panic-macro:panic:1`
names the first `panic!` in `run`. Unregistered-site diagnostics show the
current `path:line:column` (1-based) for navigation. Stale keys identify sites
which no longer exist and have no current coordinates.
Each value has `owner` (the owning package's repository path)
and nonempty `classes` (the spellings in the architecture's
[fatal/reset contract](../../../docs/architecture.md#fatal-decisions-and-reset)).
A shared dispatch mechanism can accept several classes. An entry containing
`hardware-impossible` references a bounded `SOURCE:` statement in `[proofs]`;
the checker validates the reference, not the hardware proposition. The
InvalidHart proof is conditional on the stated rev0 model, correct CSR
observation and intact code/data. It proves no radio event ordering.

Reset owners are admitted explicitly: HAL, chip composition, staged runtime,
staged bootstrap, executor and the image containing the site. The family
IEEE 802.15.4 engine's typed reasons belong to PHY HIL's terminal policy;
their declaration grants the engine no reset dependency. An ordinary driver
or sans-IO source cannot gain an owner merely by adding a registry entry.

The scan starts at library/binary roots below `chips/`, `firmware/`,
`portable/` and `protocols/`, follows out-of-line/inline modules, `#[path]`
and literal `include!` files, and considers target/feature alternatives
together. It skips `cfg(test)`, `cfg(all(test, ...))` and `#[test]` items,
const/static initializers and inline const blocks; `test-support` counts as
production. Generated PAC code and computed/generated includes are outside
the source inventory. It does not execute build scripts.

Site kinds cover:

- `terminal-entry`: synchronous functions/methods returning `!`, including
  image roots and terminal waits; a nonreturning root is not itself a fault.
- `terminal-reference`: references to fatal/reset APIs and named PHY
  reporters, including generic callback values and simple import aliases;
  qualified `fatal::reset`, fault reporters and panic dispatch are included.
- `reset-method`: every image `.reset()` call, including executor failure
  reporters. `[nonterminal]` can contain reviewed parser/frame
  resets which return normally; these use the same stable key format, each
  needs a reason and must still exist.
- `reset-trigger`: HAL system-reset requests and the typed PAC reset strobe
  used by HAL and the isolated facts image.
- `retained-reason`, `retained-code`, `retained-slot`: runtime, PHY,
  IEEE 802.15.4 and HIL reason catalogues, numeric decoder arms (including
  payload subcodes), retained slots and the facts image's `.noinit` slot.
- `terminal-binding`, `panic-handler`: image entry, exception and PHY
  reporter bindings, plus panic handlers in source and macro definitions.
- `panic-macro`, `panic-method`: explicit panic/assertion macros and image
  `unwrap`/`expect` candidates; runtime/bootstrap macro templates also count.

This is a conservative syntax inventory, not Rust name/type resolution or
macro expansion. It scans macro bodies but does not multiply them by their
expansions. Glob imports, arbitrary function-pointer dataflow, implicit
bounds/overflow/division panics, transitive panicking methods and linker/ROM
abort paths require review and the separate image analysis. The registry
does not prove that a site is reachable, that a record commits or survives
a reset, or that its observations justify impossibility. Formatting and
line shifts preserve keys; file moves, enclosing-item changes and changes
to the site itself require reviewed registry updates. Do not
regenerate classifications without checking their cause and owner.

## HAL atoms

`hal_production_code_gains_no_waits_or_sequences` scans the production
modules of `oer-esp32s31-hal` from its crate root and counts, per file, the
identifiers `async`, `await`, `Poll`, `Waker`, `poll_fn`, `loop`,
`spin_loop`, `Instant`, `BootWait` and any identifier containing
`deadline`. Each file with markers is listed with its exact count:
`HAL_SEQUENCES` with the module change that removes it
(`HAL_SEQUENCE_REMOVAL`), or `HAL_MARKERS_IN_ATOMS` with what a review found
each marker to be (a terminal halt, a register named for a deadline, a clock
reading, a re-exported driver type). A new file, a higher count, a lower
count and a stale entry all fail.

The scan is lexical and proves no absence of sequences:

- it skips items under `cfg(test)` and `cfg(all(test, ...))` only;
  `test-support` code counts as production;
- it follows `mod` declarations, including `#[path]`, but not `include!` or
  code a build script generates;
- `while` is not a marker, because const functions iterate with it, so a
  polling `while` without a deadline or time type is not found;
- a sequence of writes without a marker is not found, so a fixed vendor
  series and a series that branches on a read value look alike;
- reference counts, membership and first/last-client policy carry no marker.

The rule therefore stops new waiting code in the HAL; which code is an atom
remains a review question ([architecture](../../../docs/architecture.md#layers)).
