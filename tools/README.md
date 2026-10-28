# Tools

Everything that runs on the developer's machine, by domain. A domain holds
its libraries and its program; the layout and its rules are
[docs/repository-layout.md](../docs/repository-layout.md).

```text
tools/
  os/         operating-system bindings: processes, serial ports, durable files, toolchains
  test/       the layer test (names, unsafe boundary) and the compile-fail test support
  analysis/   binary analysis: ELF, RV32 decoding, lifting, the program model, stack analysis;
              cli/ is `blobray`
  chip/       the supported chips' profiles and stack policy
  pins/       the vendor pins: artifacts/ reads and fetches them, cli/ is `cargo pins`
  regulatory/ `cargo regdb`: generate the regulatory domain tables from wireless-regdb
  registers/  the register model and its generation; cli/ is `cargo registers`
  probe/      probe code generation and macros
  image/      the image pipeline: build, checks, comparison, ESP-IDF; cli/ is `cargo fw`,
              linker/ the image linker rustc runs
  lab/        the stand: boards, leases, power, router, fixtures; cli/ is `cargo stand`,
              fixture/ the Linux fixture contract (a library), fixture-install/ the
              privileged installer and launchers
  hil/        HIL runs, the run store, scenarios, fixtures and analyses; cli/ is `cargo hil`,
              runner/ the runner the observer builds, fixture/ the Linux helper programs,
              acceptance/ the acceptance catalog
  vendor/     the vendor comparison, lineage, provenance and the PHY audit; cli/ is
              `cargo vendor-check`, scenario-cli/ the typed vendor scenarios and reviewer commands,
              ieee802154-esp32s31-cli/ the IEEE 802.15.4 host stand
```

A program is one package: `main.rs` and its own modules. Code moves into a
library of its domain only when a second program or a library needs it. A
program called by name (a privileged helper, the linker) keeps that name
with `[[bin]] name`. The aliases in `.cargo/config.toml` run the programs.
