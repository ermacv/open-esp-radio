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
              scenario program's life cycle, cli/ is `cargo hil` (scenarios: hil/scenarios/)
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
| Run a HIL scenario | [`cargo hil`](hil/cli/README.md), [scenarios](../hil/README.md) |
| Install Linux fixture software | [Fixture contract](lab/fixture/README.md) |
| Report the missed mutants of a sans-IO pull request | [Mutation testing](#mutation-testing) |

The typed vendor scenario library is `vendor/scenario/`; its CLI is
`vendor/scenario-cli/` (`oer-tools-vendor-scenario-cli`). Host package paths
are listed in the [workspace inventory](../docs/repository-layout.md#workspace-packages).

## Mutation testing

`cargo-mutants` 27.1.0 (`cargo install cargo-mutants --version 27.1.0
--locked`) changes one operator, value or body at a time and runs the
package's tests against each change. A pull request that changes a sans-IO
crate runs it over its own diff, one package at a time, with its scratch
under the clone's `target/`, never `/tmp`:

It runs in two stages. `.cargo/mutants.toml` makes a plain `cargo mutants`
the first: the library's tests without the searches over every order of
events (`--lib -- --skip tests::search`: a module's path, so whatever is put
into a module named `search` is skipped) and without the compile-fail
cases. A search takes most of a crate's test time and `cargo mutants` runs
the tests for every mutant. The second stage runs everything, only for what
the first missed (`--iterate` keeps the mutants already caught):

```sh
mkdir -p target/mutants/tmp
git diff origin/<base>... > target/mutants/pr.diff
TMPDIR=$PWD/target/mutants/tmp cargo mutants -p <package> \
  --in-diff target/mutants/pr.diff --output target/mutants --jobs 4
TMPDIR=$PWD/target/mutants/tmp cargo mutants -p <package> --no-config \
  --in-diff target/mutants/pr.diff --output target/mutants --jobs 4 --iterate
```

A search's binaries are built with `opt-level = 1` and the overflow checks
kept, by the environment, never by a profile in the tree:
`CARGO_PROFILE_TEST_OPT_LEVEL=1 CARGO_PROFILE_TEST_OVERFLOW_CHECKS=true`, in a
`CARGO_TARGET_DIR` of their own. The engine's five named receive bounds take
62 s together that way, 160 s unoptimized.

A crate whose slow tests are not in a module named `search` gets its own
line in `.cargo/mutants.toml` when it has such tests; today it is the
engine's (`oer-protocols-ieee80211-core-datapath`) alone. A crate's ordinary
`cargo test` stays within 30 s: larger bounds of its searches are
`#[ignore]`d tests run by name once a pull request, with their time and peak
memory in it, under `ulimit -v`. A check that will not end is stopped and
reported (what ran, for how long, at what memory), never left to run behind
another.

Four jobs: over the whole engine crate (582 mutants), four took 11 min
19 s with 25 missed and 10 timed out, eight 9 min 34 s with 22 missed and
13 timed out. More jobs gained little, and the load turned missed mutants
into timeouts, whose limit is set from the unmutated baseline. Run it
without a
`CARGO_TARGET_DIR` inside the clone: with one, the compile-fail cases of the
unmutated baseline keep the copy's absolute path in their output and fail.

The pull request lists every missed mutant
(`target/mutants/mutants.out/missed.txt`) with its outcome: a test added,
the code removed, or equivalent with its reason. The list is a report,
never a gate. It does not see an order of events, so it does not replace a
crate's search or its hand-made mutants. The HAL and the hardware crates do
not run it: their host tests are thin.
