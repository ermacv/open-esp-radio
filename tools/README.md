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
              privileged installer and launchers, fixture-helpers/ the Linux helper
              programs, linux-net/ and linux-bluetooth/ their providers
  hil/        the host end of the HIL core: link/ the board's serial link, harness/ a
              scenario program's life cycle, cli/ is `cargo hil` (scenarios: hil-scenarios/)
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
| Share the stand: queue, leases, boards | [Arbiter](lab/arbiter/README.md), [board I/O](lab/board/STAND.md), `cargo stand --help` |
| Run a HIL scenario | [`cargo hil`](hil/cli/README.md), [scenarios](../hil-scenarios/README.md) |
| Install Linux fixture software | [Fixture contract](lab/fixture/README.md) |

The typed vendor scenario library is `vendor/scenario/`; its CLI is
`vendor/scenario-cli/` (`oer-tool-vendor-scenario-cli`). Host package paths
are listed in the [workspace inventory](../docs/repository-layout.md#workspace-packages).
