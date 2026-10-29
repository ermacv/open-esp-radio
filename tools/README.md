# Tools

Everything that runs on the developer's machine, by domain. A domain holds
its libraries and its program; the layout and its rules are
[docs/repository-layout.md](../docs/repository-layout.md).

```text
tools/
  os/         operating-system bindings: processes, serial ports, durable files, toolchains
  test/       the layer test (names, unsafe boundary) and the compile-fail test support
  analysis/   binary analysis: ELF, RV32 decoding, lifting, the program model;
              cli/ is `blobray`
  chip/       the supported chips' profiles and stack policy
  pins/       the vendor pins: artifacts/ reads and fetches them, cli/ is `cargo pins`
  regulatory/ `cargo regdb`: generate the regulatory domain tables from wireless-regdb
  registers/  the register model and its generation; cli/ is `cargo registers`
  probe/      probe code generation and macros
  image/      the image pipeline: bundle format, build, checks, stack analysis, comparison, ESP-IDF; cli/ is `cargo fw`,
              linker/ the image linker rustc runs
  lab/        the stand: boards, leases, power, router, fixtures; cli/ is `cargo stand`,
              fixture/ the Linux fixture contract (a library), fixture-install/ the
              privileged installer and launchers
  hil/        HIL runs, the run store, scenarios, fixtures and analyses; cli/ is `cargo hil`,
              runner/ the runner the observer builds, fixture/ the Linux helper programs,
              session/ the run-level stand operations, stats/ the bundle analyses,
              family-*/ the radio workloads, acceptance/ the acceptance catalog
  vendor/     the vendor comparison, lineage, provenance and the PHY archive audit; cli/ is
              `cargo vendor-check`, scenario-cli/ the typed vendor scenarios and reviewer commands,
              ieee802154-esp32s31-cli/ the IEEE 802.15.4 host stand
```

A program is one package: `main.rs` and its own modules. Code moves into a
library of its domain only when a second program or a library needs it. A
program called by name (a privileged helper, the linker) keeps that name
with `[[bin]] name`. The aliases in `.cargo/config.toml` run the programs.

## Choose a task

| Task | Entry point |
| --- | --- |
| Analyze a captured binary or vendor library | [Blobray](analysis/blobray-README.md) |
| Edit or generate the register model | [Register generation](registers/generate/README.md) |
| Build, inspect or run an application image | [`cargo fw`](image/cli/README.md) |
| Inspect the image artifact format | [Image bundle](image/bundle/README.md) |
| Audit a source-only PHY archive | [PHY archive audit](vendor/cli/PHY_ARCHIVE.md) |
| Fetch pinned vendor inputs | [Pins library](pins/artifacts/README.md), `cargo pins` |
| Configure and share the stand | [Stand operations](hil/run/stand.md) |
| Run a HIL scenario or inspect its evidence | [HIL](hil/run/HIL.md) |
| Install Linux fixture software | [Fixture installation](hil/run/fixtures.md#linux-fixture-software-installation) |

The typed vendor scenario library is `vendor/scenario/`; its CLI is
`vendor/scenario-cli/` (`oer-tool-vendor-scenario-cli`). Host package paths
are listed in the [workspace inventory](../docs/repository-layout.md#workspace-packages).
