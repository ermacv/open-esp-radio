# Host process ownership

`oer-os-process` supplies the process lifecycle shared by repository commands and
other host tools. It owns direct commands and their ordinary Unix process-group
descendants. Callers own workload policy, arguments, evidence and resource limits.

Install signal handlers once at the application boundary. SIGINT, SIGTERM and
SIGHUP set cancellation; new work is rejected and existing waits terminate their
owned process group. `check_cancelled` and `sleep` integrate non-process loops.

A child that needs the terminal (`foreground::Child`) stays in its parent's
process group, the terminal's foreground job: it reads the terminal, and
while it runs Ctrl-C is its own, so SIGINT cancels nothing in the parent;
SIGTERM and SIGHUP still do. `terminate` stops the child alone (SIGTERM,
then SIGKILL after a grace); what it started is its own to end.

An owner that dies without that cleanup (SIGKILL, an abort) does not leave its
children running: the first owned spawn forks a small guardian process that
tracks every owned process group through a close-on-exec pipe, and when the
pipe closes with the owner it terminates the groups still registered (SIGTERM,
then SIGKILL after a second). No owned group runs unguarded: a spawn fails,
stopping its child, when the guardian did not start, no longer reads, or
already tracks `owned::MAX_LIVE_CHILDREN` (256) live groups. `run_with_timeout` bounds a command's whole
lifetime.

`CommandExt` provides bounded status/output probes and owned background children.
Captured stdout and stderr are drained concurrently. Background captures should
set `Child::with_timeout` from the workload duration; waiting later does not
restart that lifetime. `run` preserves the caller's unlimited runtime, with
bounded shutdown. A nested supervisor can request a longer shutdown grace.
`exit_code` maps a child's status to the code a wrapper exits with: the
child's own code, or `128 + signal` for one a signal ended.

`cleanup` temporarily permits restoration despite cancellation. Its 30-second
budget is shared by nested cleanup scopes and limits subprocesses started there.
Leaving the scope restores cancellation, including during unwinding. Arbitrary
blocking code inside a closure is not forcibly interrupted.

`command(program)` creates a command with inherited operation context cleared,
including detached std spawns. Owned spawns apply the same default to a raw
`std::process::Command`. Applications attach `Context` explicitly to the child
that joins an operation, using `Context::apply`; they own the keys and choose
which fields to forward. `Context::current` parses that process's context once
and rejects malformed input. The environment is the exec transport, rather
than an application API for globally mutating child authority.

Four more jobs have their one owner here:

- `git`: every repository tool runs Git as `git -C <directory> …` through
  `git::output`, `git::text`, `git::lines`, `git::run` or, for extra
  arguments or environment, `git::command`; a failure carries Git's error
  output.
- `Checkout` and `built_root`: the checkout a tool acts on. A tool is built
  from a checkout and this package is its path dependency, so
  `built_root()` is the tree the binary came from, and
  `Checkout::discover(tool)` refuses to run inside another checkout
  (`--root` selects one explicitly).
- `lock`: `FileLock`, the one advisory file lock (`flock`), `Shared` or
  `Exclusive`, taken at once (`try_acquire`), blocking (`acquire`), polling
  cancellably (`wait`, `wait_any`) or converted in place (`convert`), and
  released explicitly on drop. Every advisory file lock of the host tools
  goes through it.
- `proc`: a live process's start time (`start_ticks`, `started_unix_millis`)
  from Linux `/proc`, which tells a recycled PID from the process that held
  it.

The implementation supports Unix process groups. Processes that create
another group or session, and processes on a remote SSH host, need their own
lifecycle owner. Integration tests use real processes to exercise signals,
pipe capacity, deadlines and daemon cleanup.

```console
cargo test -p oer-os-process
```
