# HIL experiments

`oer-tool-hil-cli` launches runs and runs experiments over them.

| Module | Owns |
| --- | --- |
| `launch` | `launch_run(&Launch) -> Launched`, the one way a run is launched: the runner executable runs supervised, with cancellation forwarded and a five-minute shutdown grace, and its runs come back from its launch run list (`OER_HIL_LAUNCH_RUNS`), never from the store's newest directories; `Runner::prepare` builds a checkout's runner and keeps its private copy |
| `job` | The arbiter job a launch runs as: recorded, started after `--after`, finished with its runs' outcomes |
| `ab` | Two firmware variants on the same scenarios: arms in detached worktrees the runner builds each arm from, rounds under whole-stand leases, the report |
| `bisect` | One step of a manual bisection for `git bisect run`: the revision in a detached worktree, an ordinary run of the scenario with this checkout's runner (`run --source`), its exit code 0 pass, 1 fail, 125 skip (the image does not build or link, or another HIL wire), 128 stop |

Revisions are checked out with `oer_os_process::git::Worktree`, the one
worktree helper, which `cargo hil images compare` shares. Nothing here runs
`cargo hil`.

A/B preparation and bisection steps forward their enclosing
lease's explicit context, replacing its job ID with the new run's job ID.
Each launch sets the context's owner to the experiment's resolved owner, so an
explicit override also wins inside another owner's lease.
Measured A/B rounds select context from their round grant. Nested runners can
therefore join the parent lease and use its delegated boards while ordinary
build and Git subprocesses receive no operation context.

```console
cargo test -p oer-tool-hil-cli
```
