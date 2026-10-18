# HIL runs and investigation

Every run leaves an immutable bundle in the shared run store. This guide covers
finding and comparing runs, what the runner records when a repetition fails and
how it brings a board back, and the tools for experiments: enqueued runs,
bisection, a revision's scenario set, A/B comparison, performance tracking, profiles and layout seeds.
The bundle contract is in the [architecture](architecture.md).

## Find and compare runs

Every checkout's `target/hil/runs` is a link to one store shared by
all checkouts of this user, `~/.local/share/open-esp-radio/hil/runs`
(`$XDG_DATA_HOME`, or `$OER_HIL_STORE/<target>/runs`). `cargo hil` creates the
link, and refuses a checkout whose own run directory holds runs.

```console
cargo hil runs list --scenario system-watchdog --outcome failed --since 7d
cargo hil runs show <run-id>          # where its artifacts are
cargo hil runs why <run-id>
cargo hil runs compare <run-a> <run-b> --measurement mbps
cargo hil runs history <scenario> --measurement mbps
cargo hil runs flaky --since 7d      # scenarios that did not always pass
cargo hil runs quarantine <scenario> --reason "loses its hello" # out of run-all
cargo hil runs release <scenario>
cargo hil runs pin <run-id> --reason "A/B baseline"
cargo hil runs prune                  # list what the rule would delete
cargo hil runs prune --apply
cargo hil runs prune --unreadable     # also this schema's unreadable runs
```

`list` shows each run's time, outcome, checkout, commit (`+` when dirty),
images and scenario outcomes, filtered by scenario, outcome, image class or
digest prefix and age. `show` prints the run's
directory in the store, its reports, and per scenario and repetition the
outcome, the artifact directory and the files in it; use it rather than
`find`, which does not follow the store link. `why` names, per failed
repetition, the recorded failure, the measurements that missed their
criteria (a criterion miss, unlike a fault), cleanup failures, host USB
events of its boards, the artifact directory and the end of `uart.log`.
Every view reads the bundles through the run bundle's typed reader and
aggregates measurements through `oer-tool-service-hil-analysis`'s one `samples`, one value
per repetition. A measurement is identified by scenario, name, unit,
`semantics` (a version its producer bumps when the value's meaning changes
under the same name) and `better`; values whose identity differs are never
combined or compared. `compare` judges each measurement of a common scenario with
the same noise-aware comparison as `cargo hil ab` (Welch's 95 % interval and
a 2 % practical tolerance, `insufficient repetitions` below three a side) and
prints both means, the relative difference and the verdict; `history` shows
the mean of each repetition set. These views never decide
qualification. `cargo hil wait RUN` follows a run's `events.jsonl`, printing
each step, until the run ends or its runner is gone, and exits with its
outcome: 0 passed, 1 failed, broken, blocked or skipped, 2 interrupted,
abandoned or on a quarantined board; an agent that started a run in the
background waits on it instead of polling its log. The same `cargo hil wait`
takes a job id (below) or `--service`. `why`, `compare` and `pin` read only the
runs they name.

## Failed repetitions and board recovery

When a repetition fails, the runner attaches to the target without resetting
it and asks for its boot evidence, waiting up to 20 s for the target's hang
watchdog to reset a stalled image. A post-mortem hang makes the failure
`hang`: which executor stalled, where each hart was, named from the run's
archived `runtime.elf` (function, the function it was inlined into, and the
source line), and the code the stalled hart kept running; a core 1 stall seen
together with a core 0 stall is reported as depending on core 0's timers.
A
reset the runner did not cause (neither USB Serial/JTAG nor JTAG) makes it
`unexpected-reset` with the reset reason and code. Both name the last
checkpoints the ended boot passed. The exchange and the workload's own
failure are kept in the repetition's `post-mortem/`.

A traffic session that ends without passing names its first failed check in
its `network::Finished` message (a typed `SessionFailure`: no datagrams, no terminal
marker, receive, socket or transmit errors, an incomplete TCP connection or
transfer, a pattern mismatch, receive loss, a failed ownership audit, or
control-link corruption), and the runner puts it in the repetition's failure
message. At the start of every repetition the runner resolves the kernel USB
device (`3-8`) behind the device's and the reference peer's serial ports; at
its end it reads their disconnects and enumerations since the start from the
kernel log (`journalctl --dmesg`) into `usb-events.json`, written only when
there are any. A board whose USB bridge dropped off the bus mid-session thus
shows as such rather than as a silent target. When the kernel log cannot be
read, the runner says so and the repetition's result stands.

When the image preflight gets no hello and the console ends in a boot loop
(at least two consecutive ROM resets of the same kind, such as `rst:0x7
(HP_SYS_HP_WDT0_RESET)` right after the second-stage bootloader), the RTS
reset and the reflash have not cleared it: firmware can leave low-power and
PMU state (a powered-down MPLL) that survives both. The runner escalates
through the board's recovery ladder (below) without its RTS rung, and after
each step asks the image for its image keys again; the first step after which
it answers clears the loop and the repetition continues. Every step and its
ROM line are recorded in the repetition's `reset-escalation.json` and a
clearing step in the board journal. When no step clears it and the ROM stays
silent, the board is quarantined with trigger `boot-loop` for a person, and
runs and `cargo stand wait --service` wait for its release; a ROM that answers
leaves the board serving, since firmware can be loaded into it.

A target that does not answer within those 20 s is first read through its
JTAG, where the stand's OpenOCD reaches it: the runner halts the current hart,
reads `pc`, `ra`, `sp`, `mcause`, `mepc`, `mtval` and `mstatus`, lets it run on
without a reset, names the code addresses from the image's ELF and writes them
to `post-mortem/jtag.json`, so the place it stopped survives the resets below.
The read never changes the repetition's outcome.

The `stand-recovery` scenario (`hil/scenarios/system/`, image
`diagnostic-usb-jtag-off`) exercises this ladder on a board that really left
USB: its image switches the USB Serial/JTAG off on request, the ladder's RTS
and JTAG rungs cannot reach it and the power rung must bring it back; then,
off USB again, the download entry must put its ROM into download mode and an
RTS reset boot the image again. `stand-recovery.json` keeps each step, how
long the board took to leave USB and how long the ladder and the entry took.

Every flash of the device under test addresses the board by its
`/dev/serial/by-id` link, which a power cycle does not change; when the board
is not on USB at all, because its image switched its USB Serial/JTAG off, the
runner first puts its ROM into download mode through its hub port's power (the
ladder's download entry) and then flashes.

A target that does not answer within those 20 s climbs the recovery ladder
(`oer_tool_service_hil_lab::control::climb`): the resets of its stand-file `reset`
ladder in order (an RTS pulse on its USB Serial/JTAG port, a system reset
through its builtin USB-JTAG with OpenOCD, whose executable the `cargo hil`
wrapper passes to the runner, its hub port's power), each followed by the
same query, then, for a board that resets by power, the automatic entry into
its ROM's download mode: its port is powered off and on and, as soon as its
USB returns, it is reset into download through the USB Serial/JTAG, before a
flashed image that switches the USB Serial/JTAG off can run. Every step is
kept in `post-mortem/recovery.json`. The failure then names where core 0 was
when the first reset hit, from the ROM banner's saved program counter
symbolized like a hang: stuck in code, or idle in its executor. A step that brings it back is journaled as a recovery, `hardware` when
the port had vanished or the ROM waited for a download. When the ROM answers
a reset, booting from flash or waiting for a download, but the firmware does
not, the failure names a firmware or host fault: the stand can reflash the
board, so it goes on serving. Once that scenario ends, the runner flashes the
chip's recovery image, `boot-smoke` built from the run's sources, checks that
it answers as itself and journals a `Reflash` recovery, so the board is left
working, once per image class and run. The flash proved its ROM answers, so a
recovery image that does not build or does not answer is the fault of that
image or the checkout that built it, never the board's: the board is not
quarantined, and the run's remaining `boot-smoke` repetitions are recorded
as `blocked`. A recovery image that does not flash proves nothing of the ROM,
so the board climbs its reset ladder, which quarantines it only if its ROM
stays silent. Once the recovery image itself proved silent, the run does not
flash it again for another image class, so the first attempt's evidence in
`session/recovery-image` stays. A quarantine the arbiter does not record ends the run with an
error, since the board would otherwise serve its remaining repetitions and the
next lease broken. The run then records its remaining repetitions of that
image class as `blocked` without touching the board, so a broken image frees
the lease within about a minute instead of repeating the wait. Only a board whose ROM stays silent after every
step of its ladder, the download entry included, which no script can bring
back to a state where firmware can be loaded, is quarantined; frequent recoveries are
shown as its health, never a quarantine. A cancelled run judges no board. On a
quarantine the repetition and the run's remaining repetitions end
`board-quarantined`,
which is no verdict on the code under test, every request for the board is
refused with the reason, and the user is notified. What the stand saw stays in
the repetition's `post-mortem/`, which the quarantine names. After pressing
the board's reset button or power-cycling it, `cargo stand devices release
BOARD --confirm reset|power-cycle` returns it once an RTS reset shows it
booting from flash; the release is journaled. `--confirm rom-answers` returns
a board nobody touched, such as one an older runner quarantined although its
ROM answered: the same RTS check must show the ROM booting.

## Operational commands without a rebuild

`cargo hil` builds its binary, [`oer-tool-cli-hil`](cli/README.md), from the
caller's working tree before every command, so an edit of a HIL host package
rebuilds it. For the operational commands (`queue`, `lease`, `devices`,
`board`, `runs`, `perf`, `dashboard`) use the installed tool instead:

```console
cargo stand install      # build origin/main's oer-tool-cli-hil once, install oer-tool-cli-stand
oer-tool-cli-stand queue
oer-tool-cli-stand runs why <run-id>
```

`oer-tool-cli-stand` runs the `main` build against the caller's checkout (its Git top
level), so owners and local paths are the caller's and every checkout writes the
shared arbiter state with one binary. Reinstall after arbiter or stand changes
land on `main`. Runs, image builds and flashing keep using `cargo hil`, which
builds the runner from the caller's sources.

## Program-counter profiles

A scenario with a [`[profile]` table](../scenarios/README.md#program-counter-profile)
arms the image's sampler right after each boot's hello; the workload opens
and closes the sampled window, and when the capture finishes the runner pages
out the raw `(pc, ra)` samples of both harts into the capture's
`profile.json`, with the target's status, and disarms it. A drain that fails
is recorded there as an error, never as a workload failure.

```console
cargo hil profile <run-id>                       # every profiled repetition
cargo hil profile <run-id> --scenario S --repetition 1 --top 20
```

`profile` symbolizes each `profile.json` against the run's own
`firmware/<image>/runtime.elf` (stored compressed as `runtime.elf.deflate`), prints each hart's most sampled functions
(the outermost frame of the inline chain), their most frequent inlined frame
and the callers the return addresses name, and writes the report to this
checkout's `target/hil/profiles/<run>/<scenario>/<repetition>/profile.txt`;
it never writes into the sealed run bundle. A return address names the caller only while the sampled
function has not called another, so callers are exact for leaf functions.

## Code layout seeds

Throughput of code that runs from cached external memory depends on where
the linker happens to place each function, so an unrelated change can move a
figure by tens of percent. `run`, `run-all` and `image build` accept `--layout-seed N` (a nonzero u32): the runtime's
ordinary code and read-only data are linked in the order that seed shuffles
them to, through the platform's `OER_LAYOUT_SEED`. Without the flag the
runner removes any inherited `OER_LAYOUT_SEED`, so the natural order is the
only unrecorded layout. The seed is part of the image: seeded artifacts get
directories of their own, the build record's parameters name it, and the
run manifest's `firmware[]` entry carries `layout_seed`, which `report
verify` requires to equal the build record's. Comparing several seeds of the
same commit separates placement from the source change under test.

## Enqueued runs

```console
id=$(cargo hil run <scenario> --enqueue)          # returns at once with a job id
cargo hil run <other> --enqueue --after "$id"     # starts once that job has ended
cargo hil wait "$id"                              # blocks until the job ends
```

`run` and `run-all` with `--enqueue` record a job in the arbiter
directory's `jobs/`, start the same command detached in its own process group
(output in `target/hil/jobs/<id>.log`), print the job's id and return. The job
moves its record from pending to started to finished, with the typed outcome
of its runs (the worst of them) and their ids. `--after JOB` makes a run,
enqueued or in the foreground, start once that job has ended with a judged
run, passed or failed; after one that ended without (no run, blocked, broken,
interrupted, abandoned) it does not start and ends no-run itself, saying why,
so a broken chain stops at its first link. `--after-any JOB` starts it
whatever the outcome. `--enqueue` first has the runner check a `run`'s
scenarios, target and options (`run --validate-only`), so a mistake such as
an unknown scenario fails in the terminal instead of in the job. It also
fixes what the job runs with before it waits: a copy of the `cargo hil`
binary below `target/hil/jobs/cli/`, the runner built now and, for a `run` that builds,
a source snapshot of the checkout taken now with the run's own
`--source-include` and `--include-untracked`. It then builds the run's images
from that snapshot and runs their audits (`run --build-only`), so a failing
build or audit ends the command in the terminal instead of the job after its
wait. Edits, pulls and rebuilds of the
checkout while the job waits or runs do not reach it; a job whose fixed parts
were deleted meanwhile fails and asks to be enqueued again. `wait JOB` blocks until the job ends and exits with its outcome:
0 passed, 1 failed, 2 interrupted (or on a quarantined board), 3 blocked or
skipped, 4 broken, 5 no run created, 6 abandoned (its process is gone without
finishing, told by its PID and start time). Every `run` and `run-all`, enqueued or in the foreground, is such a job from its start, so
`queue` and the dashboard's Preparing section show runs before they ask for
the stand: each with its phase, read from the one queue (waits for a job,
building images, waiting for the stand, holding the stand). A job is the
arbiter's ticket model: its process and the runner it starts carry the
job's id (`stand.job` in their explicit process context), and every request they
make is a ticket of that job, so the queue names each holder's and waiter's job. Building costs no balance and
takes no place in the queue. A job whose process is gone is recorded as
abandoned when the list is read. `queue` also lists the jobs that ended
without a judged run within the last hour (at most 5) with the last line of
their log, under `ended_jobs` in `queue --json`; job records are kept for a
week. `queue --json` has the jobs, with `phase`, under `jobs`; the dashboard also shows a whole-stand maintenance. An agent waits
with `wait`, never by looking for process names.

## Bisecting a scenario

```console
cargo hil bisect --good <commit> --bad <commit> --scenario <id> [--layout-seed N]
```

`bisect` searches the commits after `--good` up to `--bad` (which must
descend from it) for the first one at which the scenario does not pass. Each
tested commit is checked out, detached, in the bisection's worktree below
`target/hil/bisect/<id>/`. When its `hil/protocol/messages.lock` equals this
checkout's, it speaks this checkout's wire and this checkout's runner judges
it: `run --source-snapshot DIR` builds the revision's firmware from a snapshot
of the worktree and runs this checkout's host code and scenario. A commit with
another wire, or none named, runs its own
runner, built in the worktree, while the bisection holds a whole-stand lease;
that runner uses a private arbiter directory with a copy of the stand file
(and of the older `devices.json` registry, for revisions before it).

Every step's run is launched through `oer-tool-service-hil-experiment`'s one
`launch_run`, which learns the run from the runner's receipt, through the
same revision launch as [a revision's scenario set](#a-revisions-scenario-set):
a revision's own runner is built with `oer-tool-service-hil-observer`, and its runs reach
the shared store through the worktree's `target/hil/runs` link, or, when an
older runner keeps them in a directory of its own below `target/hil`, are
moved to the store's `revision-runs/` directory beside its `runs/`. The scenario's outcome
judges the commit: passed makes it good, failed bad. A commit whose image
does not compile or does not link (told apart from the run's archived build
log), or whose own runner does not build or starts no run, is broken: neither good nor bad, and
the search probes the untested commit nearest the middle instead. Any other
run outcome (blocked, broken, interrupted, a quarantined board) stops the
bisection, since the next steps would meet the same stand. `report.json`
records every step (commit, subject, runner, verdict and run) and the
conclusion: the first bad commit (exit 0), the commits broken revisions leave
ambiguous (exit 1), or why it stopped (exit 2). Its runs record no evidence.

## A revision's scenario set

```console
cargo hil revision --rev <revision> --scenario <id> [--scenario <id>...] [--layout-seed N]
```

`revision` runs a scenario set on one revision with the runner that speaks
its HIL wire, as a bisection step does: the revision is checked out,
detached, below `target/hil/revision/<id>/`, and the same `messages.lock`
comparison chooses this checkout's runner (with the revision's firmware from
a source snapshot) or the revision's own runner inside a whole-stand lease
with a private arbiter directory. That runner gets the private arbiter and
the owner under every environment name a revision has read them from
(`OER_STAND_ARBITER_DIR` and `OER_HIL_ARBITER_DIR`, `OER_STAND_OWNER` and
`OER_HIL_OWNER`). A run it keeps below the worktree's `target/hil` (an older
runner's per-chip run directory) is moved to the store's `revision-runs/`
directory beside its `runs/` before the worktree is removed, so it outlives
this checkout, and its `suite.json` gives the outcomes. `cargo hil runs`
does not read such a run, whose bundle is in the revision's own format; it
is found at `<store>/revision-runs/<run>`. It is
how a revision whose wire differs
from this checkout's, such as an older tag, is checked for regressions,
which `ab` cannot do. The scenarios run in the order given. With the
revision's own runner their ids and definitions are the revision's; with
this checkout's runner (the same wire) they are this checkout's, and only
the firmware is the revision's. `report.json` records the revision, its commit and subject, the
runner and the run with each scenario's outcome; it pairs no measurement
with another revision's. The exit status is 0 when every scenario passed, 1
when one failed and 2 when the revision was not judged (a scenario that
neither passed nor failed, or a runner that could not run). Its runs record
no evidence.

## A/B comparison

```console
cargo hil ab --a 'rev=main' --b 'rev=main;override:esp-hal=~/src/esp-hal' \
  --scenario system-watchdog --repetitions 3 --layout-seeds 2 \
  --order-seed 7
```

`--order-seed SEED` fixes the balanced AB/BA order of the measured rounds
(default: the experiment id), so an experiment's order can be replayed; the
seed is recorded in the report and every run's manifest.

`ab` compares two variants of the firmware on the same scenarios. A variant
is a repository revision (`rev=`, HEAD when omitted) and, after `;`, local
checkouts that replace pinned dependencies (`override:esp-hal=`, the
checkout `ESP_HAL_ROOT` names). A variant may also add or remove
runtime features of every image class it builds, `features=+usb-jtag-off,-panic-hook`:
its runs build with `cargo hil run --features`, which only an experiment
accepts, the build record keeps the delta, the preflight accepts the flashed
image by the image keys of its class's features with the delta applied (an arm
without an instrument reports fewer keys than its class), and `report
verify` checks the image's features against it. Each arm's revision is
checked out in a worktree below `target/hil/ab/<id>/` and captured with its
overrides into a source snapshot; this checkout's runner builds and runs
both, so both revisions must have this checkout's `messages.lock`. For
every layout seed `1..=K` the preparation round (round 0) runs A, then B,
building their images and warming the stand up, each run under a lease of
its own; it is recorded with the `preparation` phase and never paired. The
measured rounds `1..=N` (`--repetitions N`) replay those exact images. Each
measured round, both arms of one seed in its balanced AB/BA order, holds one
whole-stand lease, so drift of the air and the calibrations falls on both
arms, and releases it: shorter work of owners with a higher balance goes
between rounds, and the queue estimates the next
round from the rounds before it. Every run records `experiment` (its id, arm, variant: the
commit and each override's path, commit and dirtiness, and its round: layout
seed, index, phase, order and order seed) in its manifest and no evidence. A comparison takes hours, so like a run it is a job: `--enqueue`
starts it detached and prints the job id for `cargo hil wait`, and `--after
JOB` orders it after another job. Every run of an arm is a job of its own,
launched with the runner fixed when the comparison was enqueued or started,
so a pull into the checkout meanwhile cannot change the protocol the arms are
run with. A job enqueued
from a worktree without an owner of its own runs for the owner of the
checkout the worktree was added from.

The report takes one value per run and measurement (the mean over the run's
repetitions) and pairs the two arms' values of each measured round, after
checking that every run belongs to the one experiment (one id and order seed,
each arm on one variant). Per measurement it gives each arm's mean,
deviation and count, the paired difference B − A with the half-width of its
95 % confidence interval, and a verdict: `significant` (the interval excludes
zero and the difference is at least 2 % of A's mean) with the better arm,
`within-noise`, or `insufficient-repetitions` (fewer than 3 pairs); a
measurement without one complete pair is reported as `no-pairs`.
A measurement's direction is the `better` (`higher` or `lower`) its
producer declares, or its gate's; a significant difference of one without a
direction is reported as `changed` with the higher arm, never as better.
`ab-report.json` beside the worktrees
holds the variants, every run and every comparison; the command prints a
summary.

## Performance across commits

A scenario's gated measurements, those with an `at-least` or `at-most`
threshold, are its performance figures; the threshold also
fixes which direction is better.

```console
cargo hil perf report --since 7d                     # every gated scenario, per commit
cargo hil perf report <scenario> --measurement <measurement>
cargo hil perf baseline <run-id> --reason "<why this run is the baseline>"
cargo hil perf check <run-id>                        # exit 1 on a regression
```

`report` summarizes clean-commit runs per commit and code layout (`natural`
or `seed N`) as mean ± sample deviation over repetitions, marks figures past
their gate, and compares each row with the scenario's baseline. For a commit
measured with more than one layout it adds the spread of the layout means next
to the repetition noise: a spread well above that noise means placement, not
the source, moves the figure. A move in the worse direction by more than twice the
baseline's deviation and 2 % of its mean is `REGRESSED`; the same margin the
other way is `improved`. `baseline` accepts only a completed run of a clean
commit and records its network implementation; baselines live in
`perf-baselines.json` beside the store and are shared by every checkout.
Summaries of sealed runs are cached in `perf-cache/`, so a report reads each
bundle once. These views never decide qualification.

After every command that executes scenarios, `cargo hil` applies this rule
to the shared store, at most once a day, and reports what it deleted.
`flaky` counts every scenario's repetitions over the period (7 days by
default) and lists those with at least `--minimum` (3) that did not always
pass, least passing first: how many passed, how many failed as the code under
test (`failed`) and how many as the stand (`broken` or `blocked`), whether the
scenario both passed and did not (flaky), its newest failure and whether it is
quarantined. `quarantine` records a scenario with its reason in the
esp32s31 store's `quarantine.json`: `run-all` then leaves it out, and a `run`
that names it runs it with a warning; `release` takes it back. Quarantine
changes no run's outcome or evidence.

`prune` keeps pinned runs, runs whose
firmware another run replayed, runs younger than `--days` (30), incomplete
runs, the latest pass and the `--keep-failed` (5) newest failures of every
scenario and image class. A run this build cannot read goes when its manifest
names an older `RUN_SCHEMA` and it is older than `--days`: no checkout reads
it any more, and after a schema bump the newest history stays that long for
an older checkout. A run of a newer schema, or of an unknown one, always
stays: the store is shared, and a newer branch's runs are only unreadable to
this checkout. A run of this schema that does not read goes only with
`--unreadable`, which the automatic rule never passes; a newer branch's run
of the same schema may be among them. The size budget below weighs every run
that stays, readable or not, and over it also deletes an older-schema run
only its age keeps. The automatic rule also holds the esp32s31 store to
a size budget, `OER_HIL_RUN_STORE_BUDGET_GIB` (40 GiB by default): over it, it
deletes the oldest runs that only their age kept until the store fits, and
every other reason above still keeps its runs. Run by hand without `--apply` it only lists the other runs and
the bytes only they hold; hard-linked firmware shared with kept runs is not
counted. Neither the rule nor the size budget touches the store's
`revision-runs/`: a run that another revision's runner wrote in its own
format (a [revision run](#a-revisions-scenario-set) or a bisection step on
an older wire) is a record of that revision, kept for comparison; the
evidence archive packs only runs of the store's `runs/`. It stays until someone
deletes `<store>/revision-runs/<run>` by hand. Pins live in the store's `pins.json`. Pruning with `--apply`, and the
automatic rule, also delete the observer builds in the store's `observers/`
that no remaining run names and that were stored more than an hour ago; a
starting run stores its build before its manifest names it. Run verification
requires every build there to hash to its name. They delete the captures in
the host's capture store that no queued or running job holds (was fixed with,
or names with `--source-snapshot`) and that were
last written or used (a build or run from `--source-snapshot` uses it) more
than a week ago, then the source objects in the
store's `sources/` that neither a remaining run's snapshot manifest nor a
remaining capture names and that were stored or reused more than a day ago. They also delete the firmware
objects of this checkout's and every registered checkout's
`target/hil/<target>/objects/` that no run links to any more: a run's
firmware is a hard link to its object, so an object with a single link serves
no run and only saves a later identical archive a copy. Collection holds the
store's lock exclusively, and archiving holds it shared, so no run reuses an
object while it is deleted.

A pin note keeps a hand reason (`runs pin RUN --reason TEXT`) and the
evidence archive's holders side by side: `cargo hil archive ref` pins the
runs it names for its own selection (`selection-<millis>-<process>-<n>`,
written in the references file; `ref --selection SELECTION` adds runs to an
existing one), `cargo hil archive pack` hands that
selection's pins to its version, and publishing the version or `cargo hil
archive unpin VERSION|SELECTION` drops that holder alone. `runs pin` keeps
the holders, and `runs unpin` drops only the hand reason: a run an
unpublished version holds stays pinned, and only `archive unpin` releases it
(rule 5). `cargo hil archive holders` lists each holder with its runs, so a
selection never packed is found and dropped. Every change of the pins is one
read-modify-write under `pins.json.lock`, and pruning holds that lock shared
from reading the pins until its last run deletion (not while it collects the
shared observer builds and source objects), so no pin lands in between. A
command that waits for the lock says so once and ends on Ctrl-C (exit 130).
