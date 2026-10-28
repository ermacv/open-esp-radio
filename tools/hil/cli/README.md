# oer-tool-hil-cli

`cargo hil`. The workspace alias runs this package's binary
(`run --quiet --profile tools -p oer-tool-hil-cli --`). `cargo stand` is the
separate stand CLI in `tools/lab/cli`.

The binary parses one clap tree containing its host commands and the runner's
`CliCommand` (`oer-hil-run`). It launches runner commands through the observer's
built `oer-tool-hil-runner`; an unknown command is a clap error.

| Module | Owns |
| --- | --- |
| `command` | The complete command tree, owner validation, analyses, firmware catalog, `run`/`run-all` jobs and automatic pruning |
| `arguments` | Forwarding explicit clap values using the runner's grammar, including fixed images for queued jobs |
| `jobs` | `--enqueue`, `--after`, `--after-any`, fixed job artifacts, detached processes and `wait JOB` |
| `pins` | Hand pins and locked read-modify-write operations on their notes |
| `experiments` | Typed `ab` and `bisect` commands |
| `peer`, `dashboard`, `images`, `firmware_catalog` | Peer I/O, the local dashboard, image checks and comparisons, and the pinned ESP-IDF catalog |

`cargo hil --help` lists every command and `cargo hil COMMAND --help` shows
its arguments. `__command-tree` walks the same clap tree and keeps the JSON
command-node schema. `cargo stand` owns leases, boards, owners and the queue.

```console
cargo hil --owner phy run boot-smoke --enqueue --after JOB
cargo hil run boot-smoke --owner phy --root /path/to/checkout
cargo stand lease --board dut --owner phy -- sh -c 'cargo hil run boot-smoke'
```

`--owner` and `--root` are global; the runner's existing `--stand-file`,
`--board` and `--peer-board` remain global. Job options belong to `run`,
`run-all` and `ab`, after the command name. A `--` ends option parsing;
`cargo stand lease` forwards the complete command after it unchanged.

Every `cargo hil` command exits 130 when Ctrl-C cancels it, including waits
for run-store pins. Host command parser failures keep status 1; runner parser
failures keep status 2.

## Processes

- `--root PATH` (or `--root=PATH`) names the checkout to act on; otherwise the binary
  acts on the checkout it was built from and refuses to run in another one.
  The installed `oer-tool-lab-cli` (`cargo stand install`) and a job's detached
  process pass it.
- The runner locates OpenOCD of the ESP-IDF tools itself
  (`oer_lab_board::openocd::Openocd::locate`) and builds a chip's catalog
  bootloader itself, through the image pipeline. A run that is a job carries
  the job's id to the runner, so the runner's requests are the job's tickets.
- An explicit `--owner` overrides the enclosing owner in the runner's operation
  context and environment, including A/B and bisect launches. The enclosing
  lease and delegated board capabilities continue to reach the runner.
- Cancellation is forwarded to the runner's process group with up to five
  minutes for cleanup; the exit code is the runner's (or `128 + signal`).

## Tests

```console
cargo test -p oer-tool-hil-cli
```

Parser, forwarding and job tests live beside their modules.
