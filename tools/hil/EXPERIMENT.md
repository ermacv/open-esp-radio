# HIL experiments

`oer-tool-hil` launches runs and runs experiments over them.

| Module | Owns |
| --- | --- |
| `launch` | `launch_run(&Launch) -> Launched`, the one way a run is launched: the runner executable runs supervised, with cancellation forwarded and a five-minute shutdown grace, and its runs come back from its launch run list (`OER_HIL_LAUNCH_RUNS`), never from the store's newest directories; `Runner::prepare` builds a checkout's runner and keeps its private copy |
| `job` | The arbiter job a launch runs as: recorded, started after `--after`, finished with its runs' outcomes |
| `ab` | Two firmware variants on the same scenarios: arms in detached worktrees the runner builds each arm from, rounds under whole-stand leases, the report |
| `revision` | A revision's scenario set with the runner of its HIL wire: `Revision` checks a revision out and launches on it with this checkout's runner for the same `messages.lock`, otherwise with the revision's own runner under a whole-stand lease and a private arbiter; `run` records each scenario's outcome in its report, never paired with another revision's measurements |
| `bisect` | The first commit at which a scenario stops passing; each step launches through `revision::Revision` and is judged by its scenario's outcome, a build failure making it broken |

Revisions are checked out with `oer_os_process::git::Worktree`, the one
worktree helper, which `cargo hil images compare` shares. Nothing here runs
`cargo hil`: a revision's own runner is built with `oer-hil-observer` and
launched directly.

A/B preparation and current-runner revision launches forward their enclosing
lease's explicit context, replacing its job ID with the new run's job ID.
Each launch sets the context's owner to the experiment's resolved owner, so an
explicit override also wins inside another owner's lease.
Measured A/B rounds select context from their round grant. Nested runners can
therefore join the parent lease and use its delegated boards while ordinary
build and Git subprocesses receive no operation context.

```console
cargo test -p oer-tool-hil
```
