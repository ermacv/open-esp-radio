# Host process ownership

`oer-tool-base-process` supplies the process lifecycle shared by repository commands and
other host tools. It owns direct commands and their ordinary Unix process-group
descendants. Callers own workload policy, arguments, evidence and resource limits.

Install signal handlers once at the application boundary. SIGINT, SIGTERM and
SIGHUP set cancellation; new work is rejected and existing waits terminate their
owned process group. `check_cancelled` and `sleep` integrate non-process loops.

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

`lock::LockBroker` transfers an exclusive `FileLock` to an independent Linux
broker. A live owner admits authenticated `BrokerOperation` connections. Owner
loss closes admission and drains admitted connections before explicitly unlocking
the file. Native callers send a completion marker after closing their ports;
unexpected connection EOF retains the caller's kernel pidfd until its complete
thread-group exit. A graceful owner end is explicit too; owner death waits for
the owner's completed exit. Unrelated copies of its file description inherited
by a concurrent fork cannot postpone that release.
Its Linux abstract socket address uses the lock file's device/inode and UID,
so even long XDG paths fit the socket limit. Both sides check `SO_PEERCRED` for
the same effective UID; the client also checks the recorded broker PID before
sending its capability. The broker calls `listen` after fork, before acknowledging
startup, so its peer credentials identify the broker itself. Processes of that
UID are the trust boundary. The capability never appears in the address.
Closing the listener removes the address, without a
stale socket file to clean up. A failed `poll` backs off for 100 ms and probes
the sockets without blocking, so persistent errors neither spin nor prevent
owner-loss detection and I/O drainage. A polling error alone never unlocks a
device.
The child uses only async-signal-safe syscalls after fork and requires Linux
`close_range` and pidfds; setup errors fail closed. `IoLifetime::pin` explicitly
retains an admitted operation through an external command and its descendants, without
passing a lock descriptor or admission capability. Before exec, a pinned command
registers its own pidfd and waits for the broker's acknowledgement. The broker
owns that direct command through its complete exit. Such children must retain
the inherited descriptor until their I/O closes. Admission records the socket's
inode. After owner loss, the broker allows the guardian's one-second cleanup,
then scans `/proc/*/fd` for remaining same-UID lifetime holders. It sends SIGTERM,
then SIGKILL after another second, repeating the scan every 250 ms to catch new
forks. New groups and sessions do not escape this ownership. Signals use kernel
pidfds, verified against retained `/proc/PID` directory identities before use.
Holders claimed by cleanup retain those pidfds through complete exit: their
sockets can close earlier during process termination. Release requires drained
connections and completed process exits, never merely an empty process scan.
Other UIDs, PID namespaces and restricted `/proc` access can hide holders; their
connections keep exclusion until their I/O actually closes. The local owner is
not signalled by its broker; local I/O guards retain that owner themselves.
Its retained `/proc` directory identifies that exemption, so recycling the
owner's numeric PID cannot hide a new lifetime holder.
Daemons that close their lifetime descriptor before cleanup claims them can
outlive the command and carry no retained device operation. An empty lifetime
describes an operation without a device, such as discovery of an unidentified hub port.
Transferred raw lock descriptors release exclusion with their last copy,
including transient copies in other children before they close inherited FDs.

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
  it, and kernel pidfds that fence complete process exit.

The implementation supports Unix process groups; pinned hardware lifetimes also
track escaped descendants by socket identity. Processes without a pinned lifetime
that create another group/session, and processes on a remote SSH host, need their
own lifecycle owner. Integration tests use real processes to exercise signals,
pipe capacity, deadlines, daemon cleanup and inaccessible lifetime holders.

```console
cargo test -p oer-tool-base-process
```
