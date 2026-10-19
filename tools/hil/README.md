# oer-tool-hil

`cargo hil`. The workspace alias runs this package's binary
(`run --quiet -p oer-tool-hil --`); it is not part of xtask, and a stand command builds only the HIL
host packages it uses.

The binary handles the stand's own commands and forwards every other one to
the runner (`oer-tool-hil-runner`), which `oer-hil-observer` builds and
`oer-tool-hil` launches:

| Module | Owns |
| --- | --- |
| `command` | Dispatch, lease options (`--owner`), `lease`, `queue`, `owner`, `preempt`, `devices`, `wait`, `runs` and `perf` (calls into `oer-hil-analysis`), `profile`, `firmware`, the merged `__command-tree`, and `run`/`run-all`: a job, a launch through `oer-tool-hil`'s `launch_run` and automatic pruning |
| `jobs` | The command line of jobs, whose records and tickets the arbiter owns (`oer_hil_arbiter::jobs`): `--enqueue` and `--after`, what a job is fixed with below `target/hil/jobs/cli/`, its detached process, `wait JOB` |
| `pins` | A run's pin: one hand note (`runs pin`, `runs unpin`), each change one locked read-modify-write |
| `experiments` | The arguments of `ab`, `bisect` and `revision`, run by `oer-tool-hil` |
| `board`, `flash`, `stand`, `dashboard` | Arguments and calls only: board resets, consoles and soaks and the peer console through board I/O (`oer-hil-board`), manual flashing through the flash operation (`oer-hil-run`), stand discovery, doctor and fixture probes through `oer-hil-stand-host`; the live page |
| `firmware_catalog` | `cargo hil firmware`: lists and builds the ESP-IDF firmware catalog (`firmware/peers`, `firmware/vendor/<chip>`), which `oer-image-build`'s `esp_idf::catalog` builds against the pinned ESP-IDF, and flashes it through `oer_hil_flash::catalog` |

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
  The installed `oer-tool-stand` (`cargo stand install`) and a job's detached
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
cargo test -p oer-tool-hil
```

The integration tests run the real binary against a fixture checkout whose
`cargo` and runner are shell scripts.
