# HIL host setup and operations

Run commands in this guide from the repository root. HIL executes the
production driver on real hardware and records typed evidence; it is not an
alternative radio implementation. Read the
[execution and evidence architecture](architecture.md) before interpreting a
bundle.

```text
protocols/hil/{protocol,agent}/      wire protocol and chip-independent agent
firmware/hil/                      S31 HIL images
firmware/bootstrap/esp32s31/        staged bootstrap image
firmware/peers/                     ESP-IDF reference peers
firmware/vendor/                    vendor reference images

tools/hil/
  cli/          cargo hil: jobs, queries, experiments and runner launch
  runner/       executable embedding the runner's build record
  run/          runner CLI, orchestration, image and flash modules
  scenario/     catalog, campaign plan, workload context and family contract
  model/        typed scenario values, evidence schema and image classes
  observer/     runner build, dependency graph and prepared executable
  runs/         bundle format, writer, reader, store and replay
  analysis/     queries, measurement aggregation, comparisons and views
  lab/          run configuration, fixture locks, recovery and post-mortem
  link/         DUT session, protocol exchanges, captures and peer console
  family-*/     system, lower-MAC and Wi-Fi workloads
  scenarios/    versioned workloads and criteria
  fixture/      finite Linux helper binaries
  linux-net/    Linux network helper and pinned build inputs

tools/lab/      stand CLI, model, boards, router, arbiter and fixture software
tools/vendor/{phy,phy-esp32s31}/     PHY comparison and executable family
```

Target firmware lives under `firmware/hil/<chip>`. Runs are immutable bundles
in the run store (`target/hil/runs`); Markdown is not proof input.

Vendor reference images live under `firmware/vendor/`; binary-analysis
comparison code lives under `tools/analysis/blobray-verification/` and
`tools/vendor/`.

The ownership map and bundle contract are in the
[execution and evidence architecture](architecture.md).

`cargo hil` is the binary of [`oer-tool-hil-cli`](../cli/README.md).
It handles queries, jobs, experiments and the firmware catalog, and launches
runner commands through a prepared private copy of `oer-tool-hil-runner`.
`cargo stand` owns the stand's leases, boards, owners and queue. Runner
builds use `--locked`; `.cargo/config.toml` does not force offline mode.
Cancellation of `cargo hil` is forwarded to its owned runner process group, with up to five minutes
for fixture cleanup and finishing the run; this does not limit campaign runtime.
The wrapper returns the runner's exit code (or `128 + signal` on Unix).

Build the observer without running HIL, printing its private copy, with:

```console
cargo hil observer
```

## Guides

| Guide | Content |
| --- | --- |
| [Sharing the stand](stand.md) | Leases, owners and balances, boards and their reset paths, the ESP-IDF firmware catalog, host fixture probing, the dashboard |
| [Runs and investigation](runs.md) | The shared run store, failure post-mortems and board recovery, enqueued runs, bisection, revision runs, A/B comparison, performance, profiles and layout seeds |
| [Host fixtures](fixtures.md) | Current fixture-provider scope, Linux fixture software installation and helper interfaces |
| [Execution and evidence architecture](architecture.md) | Package ownership, the run lifecycle and the bundle contract |
| [Stand arbiter](../../lab/arbiter/README.md) | The queue, claims, balances and state files |

## Configure the stand

The stand file is shared by this user's checkouts:
`~/.config/open-esp-radio/stand.toml`, or under `$XDG_CONFIG_HOME`.
`--stand-file PATH` selects another file. `oer-lab-model` owns its schema,
board-name resolution and state paths. Scenarios contain no laboratory
credentials or machine-specific paths.

```console
mkdir -p ~/.config/open-esp-radio
cp tools/lab/model/stand.example.toml ~/.config/open-esp-radio/stand.toml
chmod 0600 ~/.config/open-esp-radio/stand.toml
```

A DUT is a board with the `dut` role and the selected chip. `--chip CHIP`
selects the chip; otherwise the runner requires exactly one chip capable of
building the selection. `--board ID` resolves a DUT when several qualify.
The peer-board resolver uses the declared peer image's chip and the `peer`
role; `--peer-board ID` selects among matching boards.

ESP-IDF peer projects are in `firmware/peers/`, with image names and chip
pins in their `firmware.toml` catalog entries. The runner's peer image port
uses `oer_hil_run::flash::catalog`; its `PeerConsole` is in `oer-hil-link`.
Catalog building and automatic writes are described in the
[stand guide](stand.md#esp-idf-firmware-catalog).

## Build and run

Source-only catalog operations do not open the stand file or a device:

```console
cargo hil scenario list
cargo hil scenario validate
cargo hil plan boot-smoke
cargo hil plan --proof lower-mac-conformance
```

`plan` prints procedure identity, image, repetitions, requirements and named
checks. Repeated `--proof` arguments require every named check. Lower-MAC and
Wi-Fi families supply checks including `lower-mac-conformance`,
`lower-mac-ownership-close-gate` and `wifi-station`. The plan
neither runs the workload nor infers a passing observation from tags.
Scenario identity and discovery are described by the
[catalog](../scenarios/README.md).

These commands have build or hardware effects:

| Command | Effect |
| --- | --- |
| `cargo hil observer` | Build and prepare the host runner only |
| `cargo hil image build CLASS` | Build the selected firmware class and its audited image bundle, without flashing |
| `cargo hil images check --all` | Build available HIL classes and run their image checks |
| `cargo hil doctor SCENARIO` | Inspect the selected machine and fixture prerequisites; no reservation or DUT reset |
| `cargo hil run SCENARIO` | Build and archive images, acquire the stand, flash and execute the workload |
| `cargo hil run SCENARIO --images DIR` | Use a prepared image directory after checking its identities |
| `cargo hil run SCENARIO --firmware-from RUN_ID` | Replay archived firmware after validation |

A successful image build prints a JSON artifact report with its class, chip,
profile, bundle, ELF, application identity, reports and warnings; diagnostics
go to stderr. A leftover ELF beside a failed build is not a successful
artifact report. App bundles under `target/fw/` are selected separately by
`cargo fw flash IMAGE --board BOARD`.

`run --source DIR` selects the image source tree. `--images` and
`--firmware-from` are explicit artifact inputs and exclude source rebuilding
options. `--repetitions N` overrides the catalog count within 1–20, retaining
the requested count in the run. `run-all` supports role and tag selection,
and requires `--all` for an unfiltered whole-catalog run.

An enqueued run builds its fixed image directory before queueing; the job
flashes those artifacts even if the checkout changes while it waits. Queueing,
experiments and cleanup are covered in [runs and investigation](runs.md).
Cancellation is forwarded to the owned runner process group, with up to five
minutes for cleanup; the wrapper returns the runner's exit status.

## Inspect evidence

The shared run store is `$OER_HIL_STORE` or
`<XDG data>/open-esp-radio/hil`. `target/hil/runs` links to its `runs/`
directory. Every invocation keeps its manifest, selected plan, archived
firmware, suite, scenario results and repetition attachments. JUnit and HTML
are derived views; Markdown is not proof input.

`runner.observer` records the executable and its embedded build/source
identity. The [observer input registry](../model/observer-inputs.json)
selects each workload's relevant dependency closure and data inputs.
Firmware identity does not replace the observer identity.

```console
cargo hil runs list
cargo hil runs history boot-smoke
```

Queries read the typed bundles through `oer-hil-runs::format::RunBundle`.
A different run schema is refused and left in the store. See the
[bundle format](../runs/README.md) and
[execution lifecycle](architecture.md#one-run-lifecycle) before interpreting
an outcome or replaying firmware.
