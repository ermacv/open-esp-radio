# oer-hil-cli

`cargo hil`. The workspace alias runs this package's binary
(`run --quiet -p oer-hil-cli --`); it is not part of xtask, and a stand command builds only the HIL
host packages it uses.

The binary handles the stand's own commands and forwards every other one to
the runner (`oer-hil-runner`), which `oer-hil-observer` builds and
`oer-hil-experiment` launches:

| Module | Owns |
| --- | --- |
| `command` | Dispatch, lease options (`--owner`), `lease`, `queue`, `owner`, `preempt`, `devices`, `wait`, `runs` and `perf` (calls into `oer-hil-analysis`), `profile`, `firmware`, the merged `__command-tree`, and `run`/`run-all`: a job, a launch through `oer-hil-experiment`'s `launch_run` and automatic pruning |
| `jobs` | The command line of jobs, whose records and tickets the arbiter owns (`oer_hil_arbiter::jobs`): `--enqueue` and `--after`, what a job is fixed with below `target/hil/jobs/cli/`, its detached process, `wait JOB` |
| `archive` | `cargo hil archive ref`, `pack`, `holders`, `unpin`, `publish` and `fetch` (`oer-hil-evidence-archive`): run references, pinned at once for their own selection, a version of the cited runs (the pinned version's from its downloaded copy) that takes over the selection's pins until published, `unpin` for a version never published or a selection, its release through `gh` with the pin written, and a verified copy of the pinned version; `pack`, `publish` and `fetch` reach the network. `pins`: a hand reason and the archive's holders side by side in one note, each change of the pins one locked read-modify-write; exit status 1 for a broken archive rule, 2 usage, 3 the environment, 130 cancelled |
| `experiments` | The arguments of `ab`, `bisect` and `revision`, run by `oer-hil-experiment` |
| `board`, `flash`, `stand`, `dashboard` | Arguments and calls only: board resets, consoles and soaks and the peer console through board I/O (`oer-hil-board`), manual flashing through the flash operation (`oer-hil-flash`), stand discovery, doctor and fixture probes through `oer-hil-stand-host`; the live page |
| `firmware_catalog` | `cargo hil firmware`: lists and builds the ESP-IDF firmware catalog (`hil/peers`, `verification/<chip>/hil-vendor`), which `oer-image`'s `esp_idf::catalog` builds against the pinned ESP-IDF, and flashes it through `oer_hil_flash::catalog` |

Every `cargo hil` command exits 130 when Ctrl-C cancels it, a wait for the
run store's pins included (`main.rs`); `cargo hil archive` also exits 1 for
a broken archive rule, 2 for a usage error and 3 for a failure of the
environment.

Commands and flags are documented in the [host guide](../README.md), the
[stand guide](../stand.md) and the [runs guide](../runs.md); `cargo hil --help`
prints them.

## Processes

- A leading `--root PATH` names the checkout to act on; otherwise the binary
  acts on the checkout it was built from and refuses to run in another one.
  The installed `oer-stand` (`cargo stand install`) and a job's detached
  process pass it.
- The runner locates OpenOCD of the ESP-IDF tools itself
  (`oer_hil_board::openocd::Openocd::locate`) and builds a chip's catalog
  bootloader itself, through the image pipeline. A run that is a job carries
  the job's id to the runner, so the runner's requests are the job's tickets.
- An explicit `--owner` overrides the enclosing owner in the runner's operation
  context and environment, including A/B and bisect launches. The enclosing
  lease and delegated board capabilities continue to reach the runner.
- Cancellation is forwarded to the runner's process group with up to five
  minutes for cleanup; the exit code is the runner's (or `128 + signal`).

## Tests

```console
cargo test -p oer-hil-cli
```

The integration tests run the real binary against a fixture checkout whose
`cargo` and runner are shell scripts.
