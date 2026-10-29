# HIL execution and evidence architecture

The host runner's ownership and evidence bundle contract. Operational setup
and commands are in the [host guide](HOST.md). Paths in this table are
relative to `tools/`.

| Package or module | Role |
| --- | --- |
| `hil/cli` (`oer-tool-hil-cli`) | `cargo hil`: queries, jobs, experiments, firmware catalog and runner launch |
| `hil/runner` (`oer-tool-hil-runner`) | Thin executable embedding its host build record and calling `oer_hil_run::main` |
| `hil/run` (`oer-hil-run`) | Runner CLI, selection, preflight, leases, repetitions and finishing a run; `oer-hil-run::image` builds image classes through the image pipeline, and `flash` orchestrates leased writes |
| `hil/scenario` (`oer-hil-scenario`) | Scenario envelope, catalog and campaign plan; `workload` owns the repetition context, result writer, measurements, fixture and family contracts |
| `hil/model` (`oer-hil-model`) | Catalog vocabulary, evidence schemas, image classes, feature recipes and target agent metadata |
| `hil/session` (`oer-hil-session`) | Run configuration, fixture locks and software leases, laboratory provenance, recovery and post-mortem; implements the link's DUT port |
| `hil/link` (`oer-hil-link`) | DUT session, UART capture, protocol exchanges, reboots and peer console |
| `hil/runs` (`oer-hil-runs`) | Bundle format and reader in `format`, writer in `run`, run store, object store, launch records and replay |
| `hil/stats` (`oer-hil-stats`) | Bundle queries, measurements, comparisons, retention, performance and JUnit/HTML views |
| `hil/family-system` (`oer-hil-family-system`) | System workloads: boot smoke, watchdog and panic reset |
| `hil/family-lower-mac` (`oer-hil-family-lower-mac`) | Lower-MAC conformance and ownership-close checks |
| `hil/family-wifi` (`oer-hil-family-wifi`) | W4 station qualification and the initial-GTK investigation |
| `hil/family-phy`, `hil/family-phy-esp32s31` | PHY comparison port, vendor reference driver and executable S31 PHY family |
| `lab/cli` (`oer-tool-lab-cli`) | `cargo stand`: stand leases, queue, boards, owners and fixture installation |
| `lab/model` (`oer-lab-model`) | Stand file, board-name resolution and state paths |
| `lab/board` (`oer-lab-board`) | Leased board I/O, device access, image receipts, writes, starts, resets, OpenOCD and consoles |
| `lab/arbiter` (`oer-lab-arbiter`) | Claims, queue, tickets, balances, preemption and board journal |
| `lab/router` (`oer-lab-router`) | Host discovery, router operations, SSH and pinned network-helper build inputs |
| `lab/fixture` (`oer-lab-fixture`) | Linux fixture provisioning, admission and finite helper request/report contracts |
| `lab/fixture-install` (`oer-tool-lab-fixture-install`) | Privileged installer and fixed launchers, without a library target |
| `hil/fixture` (`oer-tool-hil-fixture`) | Finite `open-radio-bluetooth` and `open-radio-probe` binaries, without a library target |

The runner implementation is `tools/hil/run/src/`: `command.rs` owns
startup and dispatch; `execution/orchestration.rs` owns selection and the
suite/scenario/repetition lifecycle; `execution/firmware.rs` coordinates
build or replay publication; `execution/preflight.rs` checks selection and
that the flashed image reports every key the scenario's family requires; `execution/doctor.rs` reports prerequisites. The thin
`tools/hil/runner/src/main.rs` embeds the build identity.

`tools/hil/run/src/scenario.rs` composes system, lower-MAC, Wi-Fi and PHY
families. Its `Families::FIXTURES` registry is empty. The workload APIs are
in `tools/hil/scenario/src/workload/`, including the machine JSON descriptor
in `output.rs`. `Context::finish` writes a repetition's
`observations.json`; `oer_hil_runs::run::RunSession` writes the run bundle.

Fixture installation has a separate ownership boundary. The unprivileged
operator builds and prepares the bundle; the narrow
`open-radio-fixture-install` binary activates fixed Linux destinations
through `oer-lab-fixture`. Provider software leases and installation
recovery are described in the [fixture guide](fixtures.md#linux-fixture-software-installation).
They do not replace the run's physical fixture or device leases.

## One run lifecycle

The runner resolves the selected catalog entries, checks image compatibility
and creates a unique run directory. Current-source images are built and
archived before waiting for the stand. `--images DIR` uses a prepared image
directory; `--firmware-from RUN_ID` uses archived firmware after replay
validation. These inputs are mutually exclusive with source rebuilding
options in the CLI grammar.

Under the arbiter's grant, the run takes its device and fixture locks and
records secret-free laboratory provenance. Writes go through the
[flash operation](FLASH.md) and its complete-bundle receipts. The runner
executes repetitions with the selected family's workload, retains partial
observations on errors, finalizes captures and cleanup, and finishes the
suite through `RunSession`. Failed repetitions may invoke the laboratory's
post-mortem and reset ladder; recovery and quarantine are recorded separately
from the scenario result. See [runs](runs.md#failed-repetitions-and-board-recovery).

| Owner | Durable result |
| --- | --- |
| `oer-hil-runs::run::RunSession` | Run directory, manifest, plan, firmware archive, suite and lifecycle events |
| `oer-hil-scenario::workload::context::Context` | Repetition observations, measurements and cleanup evidence |
| `oer-hil-link::SerialCapture` | Raw UART bytes, decoded protocol records and typed link health |
| `oer-hil-stats` | JUnit/HTML views and comparisons read from the typed bundle |
| `oer-lab-board::image` | Receipt of the complete image written and started on a board |
| `oer-lab-arbiter::journal` | Board-operation history |

## Commands and catalog

The executable grammar is in `tools/hil/run/src/cli.rs`; `cargo hil` combines
it with the host wrapper's queries, jobs and experiments.

```console
cargo hil scenario list
cargo hil scenario validate
cargo hil plan boot-smoke
cargo hil image build boot-smoke
cargo hil run boot-smoke
cargo hil run-all --role qualification
```

`scenario` and `plan` read the catalog without hardware or image builds.
`image build` runs the image pipeline. `doctor` checks the selected
prerequisites; its observations do not reserve a resource. `run` and
`run-all` acquire the stand and execute workloads. Command details and effects
are in the [host guide](HOST.md).

The recursive [catalog](../scenarios/README.md) contains one common header
and one family table per TOML document. The family validates its parameters
and supplies the plan, requirements, named checks and executable workload.
A scenario PASS records that workload's criteria; it does not independently
qualify every capability of the radio.

## Bundle and evidence

The shared run store is `$OER_HIL_STORE`, or
`<XDG data>/open-esp-radio/hil`; `target/hil/runs` links to its `runs/`
directory. `oer-hil-runs::format` defines the versioned documents and typed
reader, while `oer-hil-runs::run` writes them. Readers reject a different
run schema rather than interpreting it as current evidence.

```text
manifest.json          invocation, repository, host, lab and firmware identity
plan.json              selected catalog entries
events.jsonl           run lifecycle events
suite.json             suite, scenario and repetition outcomes
lab-provenance.json    secret-free laboratory observations
firmware/              archived application, ELF and build records
scenarios/             scenario snapshots, repetitions and their attachments
junit.xml, report.html derived views
```

A run archives the firmware bytes it uses. Replays validate archived subjects
and their identities; a Markdown narrative, derived view or previous PASS
cannot replace the underlying records. See [run bundles](../runs/README.md)
for the format and store, and [analysis](../stats/README.md) for queries.

`SerialCapture` saves received bytes incrementally as `uart.bin` and a lossy
text view as `uart.log`; `protocol.jsonl` records decoded events, sent-command
identities and link failures. Sent commands retain request/session identity
and host time, without credential-bearing payloads. Protocol observations
and workload measurements are retained even when later work fails.
Ordinary finalization and cancellation run cleanup; abrupt process death can
leave only the incrementally written files. Process ownership and shutdown
are provided by [`oer-os-process`](../../os/process/README.md).

## Source ownership

The runner library depends on the family crates, scenario APIs, laboratory,
link, image pipeline and bundle libraries. The families use
`oer-hil-scenario::workload`; the scenario library depends on the laboratory,
link and bundle libraries, which do not depend on the runner.
`oer-hil-session` implements the link's DUT port. `oer-hil-runs` depends on no
board or image builder; consumers read through
`oer_hil_runs::format::RunBundle`. The observer's producer modules are in
`oer-hil-observer`, while readers use its identity data in
`oer-hil-runs::format::observer`.

`oer-hil-scenario`, `oer-hil-session`, `oer-hil-link` and `oer-hil-runs` expose
test doubles through `test-support`. The recursive
[catalog contract](../scenarios/README.md) has one discovery,
`oer_hil_scenario::catalog::documents`; the runner interprets each family's
executable table. Tests live beside their owning modules.
