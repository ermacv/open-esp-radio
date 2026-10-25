# Sharing the HIL stand

Every checkout of this user shares one stand: its boards, host fixtures and
air. This guide covers leases, boards and their firmware; the
[host guide](HOST.md) covers configuration, building and running, and
[runs and investigation](runs.md) what a run leaves behind.


The [arbiter](../../lab/arbiter/README.md) orders every use of the stand. A
lease claims the resources its work uses: boards by MAC, fixtures such as the
laptop radio or the OpenWrt host, and the air, shared by all radio work and
exclusive for scenarios tagged `air-exclusive` that measure the radio
environment. Leases whose claims do not conflict run in parallel; conflicting
requests are served by their owners' balances (below). Hardware commands
(`run`, `run-all`, `fixture check` and `flash`) wait for their claims instead
of failing when they are busy. A run builds its images before it queues, so
the lease covers flashing and execution only.

```console
cargo stand queue                       # holders, queue with expected starts, board state
cargo hil dashboard                   # the same, live, with recent runs: http://127.0.0.1:8765
cargo hil run <scenario>              # one run, one lease
cargo hil run a b c                   # one run of three scenarios, one lease
cargo stand lease --board esp32c5 -- idf.py -p <port> flash
cargo stand --owner phy lease --board esp32s31 -- sh -c 'cargo hil run a --firmware-from R && cargo hil run a'
```

A leased command that is itself a stand command (`cargo hil …` or
`oer-tool-lab-cli …`) has its arguments parsed as that command will before the lease
queues, so a mistyped option fails at once instead of after the wait; commands
the runner parses (`run`, `run-all`, `image`) are checked when they run.

Before starting any leased command, the lease admits I/O for all its selected
boards and pins those operations into the child process. External flashers need
no repository broker API: board exclusion survives the lease owner's death
through the command's complete exit and its descendants' inherited lifetimes.
Each direct command registers its kernel pidfd before exec and waits for the
broker's acknowledgement before it can start I/O.
The process guardian sends SIGTERM to the command's process group on owner loss,
then SIGKILL after one second. This is the cleanup window, not an extension of
the lease: the broker retains exclusion until the actual I/O lifetimes end.
The broker then finds remaining same-UID holders of those descriptors through
`/proc/*/fd`, including daemons that called `setsid`. It sends SIGTERM and, after
another second, SIGKILL, repeating the search to catch forks during cleanup.
Board release requires connection EOF and completed exit of the direct command
and every holder claimed by cleanup. A socket can close before the remaining
I/O descriptors during process termination; the pidfd fences that interval.
An empty process scan cannot release the board. Other UIDs, PID namespaces and
restricted `/proc` access can prevent cleanup; the board stays busy until those holders close
I/O. The holder diagnostic names the draining broker after owner exit. A daemon
that closes its inherited lifetime descriptor before cleanup claims it can
survive the lease and carries no retained board operation.

A manual flash outside the runner is the dev kit's: `cargo fw flash
--device MAC|PORT IMAGE` writes an image bundle (its directory, from `cargo
hil image build`) or an ESP-IDF catalog image (built first) to an attached
board. It takes the
board's device lock, which a stand lease of another process holds, so it
fails at once on a leased board (or waits with `--wait`) and never writes
under a run; the write is the receipted image write of `oer_lab_devices::image`,
and `--monitor` reads the console afterwards. Over JTAG only the runner's
flash operation writes (an image that breaks USB Serial/JTAG resets, see
[Hardware errata](../../../docs/hardware-errata.md)). The OpenOCD
build comes from the ESP-IDF tools in the shared cache. The lease ends with
the capture, so an open monitor never holds a board.

### The flash operation

Every flash of a stand board by HIL, the runner's flash
of its device under test and of the reference peers and the calibration
cross-check, is one operation (`oer-hil-run`):

1. **lease**: the board is claimed in the arbiter and, once granted, its
   lock file taken;
2. **write**: the bundle's segments go through one espflash connection for
   the chip its profile names (`chips/<chip>/platform/chip.toml`), the OTA
   selection last, a segment whose flash already matches skipped after an
   MD5 comparison, a failed serial link retried up to three times; a board
   that is not on USB, because its image switched its USB Serial/JTAG off,
   is first put into the ROM's download mode through its hub port's power;
3. **journal**: the board journal records the image name, the SHA-256 of its
   application, its commit and whether the tree differed, and who wrote it;
4. **start**: as the chip profile's `[flash] start` says. `reset`: the
   writer's hard reset started it (esp32s31). `power-on`: the writer leaves
   the ROM in its download mode and a power-on reset of the board's hub port
   starts the image, an RTS reset on a board that does not reset by power
   (esp32c5): after the download mode an RTS reset starts an esp32c5's
   application with a silent console that later RTS resets do not revive
   (the host sees EOF or `EPROTO` on the port); after a power-on reset RTS
   resets work.

```console
cargo fw flash --device <MAC|PORT> --monitor <bundle-directory>
```

`dashboard` serves a page on the loopback interface (`--port` changes the
port) that refreshes every two seconds without disturbing a selection, an
open tooltip or a scroll position. A summary line says whether the stand is
held or free, the queue, board and fixture health and the last run's outcome;
when the dashboard stops answering, a banner says so and the page dims. Holders
show their time held against the estimate and the one-hour hard limit, their
run's scenarios and progress and their claims as boards, fixtures and air
ranges; the queue comes in service order with expected starts, then every
owner's balance on a scale around zero, every board's port and last flash
(linked to the run that flashed it), the host fixtures (a blocked radio is a
warning) and the newest runs of the shared store with their owner, scenario
outcomes (with symbols, not colour alone) and buttons that copy the run ID and
`cargo hil runs why` or `show`. Runs filter by owner, checkout and scenario,
and repeats of the same result fold into one row. A run's owner and leases
come from the leases that name it: the runner names its run in its lease
request, and the holder and its history record keep it. A run started inside
an enclosing lease (`cargo hil ab`, `cargo hil bisect`,
`cargo stand lease -- …`) joins that lease without a request of its own and shows no owner. Links to
a run land on its row, or on the row its repeats fold into. Recent leases hide a
released lease of a run the filtered table lists unless asked and show the time charged and the
balances the grant was decided by. Commands appear without their build-input
paths (the full line is the tooltip), a running run whose runner died shows as
abandoned and an `ab` arm is marked. A run links its `report.html`, whose
artifact links the dashboard serves from the run's directory, and an enqueued
job links the end of its log. It only reads the arbiter's state, run
bundles and job logs; Ctrl+C stops it. One dashboard serves the
host and names itself in `dashboard.json` of the arbiter directory: starting
the same build again prints the running one's address, and another build
stops it and takes over. A dashboard exits once another replaced it or once
the stand's state has a schema newer than it reads.

`run --then 'COMMAND'` runs a shell command after the scenarios while the run
still holds its lease, so nobody flashes the board in between: read the reset
reason or RTC memory after a failure, or attach OpenOCD. The command may use
the boards' ports directly, nested `cargo hil` commands join the lease, and
`OER_HIL_RUN_DIRECTORY` names the run bundle. Its exit status is reported and
recorded as a `then-succeeded` or `then-failed` event, never as the run's
outcome.

```console
cargo hil run system-watchdog --then 'python tools/read-rtc.py /dev/ttyACM0'
```

`run` accepts several scenarios: it builds every needed image class before
queueing and executes all scenarios in one run bundle under one lease, in the
given order within each image class. With `--firmware-from` every named
scenario must use the replayed image class. Prefer it to a shell loop under
`lease`, where each nested run builds while the stand is held.

`lease` runs one command under one lease; every `cargo hil` command inside it
joins that lease when the lease already holds what it needs, so no other owner
can flash between the runs of a series. `--board NAME|MAC` (repeatable) and
`--air shared|exclusive` name what the command uses. A lease naming neither is
refused: a whole-stand lease blocks every other owner, so it must be asked for
with `--stand`. A lease holds the device lock of every board it reaches, so
it is refused, before it queues, when it cannot name one: a whole-stand lease
whose stand file does not load or lists a board without a MAC, or a board
claim that is not a MAC. The lease options precede the HIL command or follow
`lease`; a runner command (`run`, `run-all`, ...) also takes them among its
arguments before `--`, and refuses two different owners:

| Option | Meaning |
| --- | --- |
| `--owner NAME` | Who holds the lease; defaults to an enclosing lease's owner, then the owner registered for the checkout |

The environment variable `OER_HIL_OWNER` carries the same choice. A lease
belongs to one of the agents that use the stand: `stand`, `wifi`, `phy`,
`bluetooth`, `blobray`, `infra`, `802154`, `esp32c5` or `network`. Each checkout
registers its owner once with `cargo stand owner set NAME`, kept in
`owners.json` of the arbiter directory; `cargo stand owner` prints it. Nothing
is derived from the checkout's directory name: a lease requested for no
agent is refused with an error naming `cargo stand owner set`. `cargo stand owner
merge OLD NEW` charges an old name's balance and history to its agent, and
`cargo stand owner forget NAME` drops a balance that belongs to no agent. A request
names no duration: the stand charges the time a lease holds, as follows.

Every lease charges its owner's balance the time it holds, and an owner whose
request waits because of another owner, behind that owner's lease or a
request of that owner served first, is credited the time it waits, once
however many requests it queued. A request waiting behind its own owner's
lease earns nothing, so queuing more work behind a running lease does not
offset its charge. Among conflicting requests the owner with the highest
balance is served first, the earlier request on a tie. Balances halve every
two hours and stay within an hour either way, and every settlement subtracts
the mean over the owners active in the last day, so balances move smoothly and
their sum stays near zero: an idle owner drifts up while others hold and down
while others wait. `cargo stand queue` and the dashboard show every balance
and who is served next; the lease history records each lease's charge, its
owner's balance at release and the balances its grant was decided by.

A run of several scenarios that has held its lease for ten minutes finishes
its current scenario when a waiting request that needs its resources belongs
to an owner with a higher balance: it releases the lease, queues again and
continues its remaining scenarios in the same run bundle after flashing its
image again. Such a run also renews its lease itself once it has held it for
20 minutes, a third of the hour below: at the next scenario boundary it
releases, queues again (at once when nobody waits) and flashes its image
again, so a long suite such as `run-all --role qualification` never meets the
hour, and a scenario started after a renewal has 40 minutes. A single scenario
and a `lease` command run to their end. Every
lease ends at one hour: it is stopped with `SIGTERM`, which runs the ordinary
cancellation and fixture cleanup, a `lease` stopped this way exits with status
124, and `SIGKILL` follows five minutes later. `cargo stand preempt ID --reason
TEXT` stops another owner's lease the same way at once: its owner is charged
no longer from that moment, the holder prints who preempted it and why when it
releases, the history records it as `preempted-on-request`, and the user is
notified. Estimates from earlier leases of
the same work only predict when a waiting request starts. `cargo hil doctor`
reports conflicting holders without queueing.

A command started in the background returns when its lease ends; its exit is
the notification for an agent. The user receives desktop notifications through
`notify-send` when a waiting owner is granted a lease, when the stand becomes
free, and when a lease reaches the hard limit or is preempted;
`OER_HIL_NOTIFY=0` disables them.

Do not wrap `cargo hil` in `timeout`: its clock also runs while the request
waits in the queue, which can last longer than the work itself. A timeout that
fires while the request waits only drops it from the queue; one that fires
during the lease stops the work with `SIGTERM` and the ordinary cleanup, so a
run ends interrupted with no verdict. Every lease already ends at the one-hour
hard limit; to stop a lease that is stuck, use `cargo stand preempt`.

The stand holds several equal boards, currently an ESP32-S31 and an ESP32-C5.
No board has a fixed role: a scenario or other consumer chooses which board it
uses as its device under test or as a peer, and may flash its own firmware.
The stand file (`~/.config/open-esp-radio/stand.toml`,
`oer-lab-model`) describes the hubs and
the boards: each board's id, chip, radios, roles, hub port and reset ladder.
Each board is identified by the MAC address its USB Serial/JTAG port reports
as USB serial number, independent of `/dev/ttyACM*` numbering. Every board is part of the
stand: flash and use any board only under a lease.

## The stand file

`cargo stand discover` maps every Espressif board `uhubctl` reports to
its stand-file hub, port and touch button and compares the result with the
stand file: `ok` for a board on its port, `moved`, `missing` with why its
port is empty (the port is off; the port is powered but empty, so its button
is off or nothing is plugged in; another device is there; its hub is not
attached) and `new`, with a `[[board]]` fragment to paste. It never changes
the file. `--blink HUB:PORT` switches a switchable port off for five seconds
under a lease of the port and of its board, to see which button it is; a
protected port, which a cascaded hub hangs on, and a port `uhubctl` cannot
cut are refused. `--verify-power BOARD` cycles the board's port under its
lease and requires the board's own USB device to leave while the port is off
and to return once it is on, and then a power-on reset read through the
chip's JTAG. A port that only drops the board from the bus keeps it powered:
the board returns with the cause of its previous reset. The ROM prints its
reset reason before the board's USB enumerates, so the stand reads the
chip's reset-cause field instead (`[reset-cause]` of its chip profile, a
field of the chip root `chips/<chip>/registers/chip.toml`); the read halts the
core for a moment and lets it run on. Only the verification of a port takes
that proof: the stand's own power cycles (the power-on start of a written
image, the download-mode entry of the reset ladder) require the board to leave
and return, and their outcome shows on its own, since the image they start may
switch the chip's JTAG off or reset again before its cause is read. A port
that has not passed `--verify-power` is not switchable.

`cargo stand doctor` checks the host around the stand file: the file
(private, valid, every board's chip and radios against its profile), the
repository's udev rule installed (`stand/udev/`) and `uhubctl` reading
every stand hub without sudo, and NetworkManager leaving `wlan0` to the
fixtures (`stand/networkmanager/`, installed in
`/etc/NetworkManager/conf.d/`).

```console
cargo stand discover              # ok s31-a on rsh-mid:3 (button 6)
cargo stand discover --blink rsh-bottom:2
cargo stand discover --verify-power s31-a
cargo stand doctor
```

## Boards

A board, or the whole stand, can be taken out of service: `cargo stand --owner
NAME devices maintenance BOARD|--stand --reason TEXT` records it in
`maintenance.json` of the arbiter directory. Until `cargo stand devices release
BOARD|--stand`, a request by another owner that claims the board (any board,
for `--stand`) or the whole stand does not get it: a run waits, printing the
reason, and starts once it is back in service; a tool acting on the board
itself (`board`, `lease`, `flash`) is refused with the reason at once. A
quarantined board is out of service the same way. A lease already held runs
on. `cargo stand queue` and the dashboard list what is out of service.

An agent that needs the stand waits for it with a shell command, never for a
chat message: `cargo stand wait --service [BOARD...]` blocks until the named
boards (every board when none is named) and the stand are back in service,
printing what it waits for, and exits 0.

`cargo stand devices`, `cargo stand queue` and the dashboard show each board's
health from the stand's own records, without touching the board: `ok`,
`recovered recently` (the stand recovered it within the last hour, with how
many of those recoveries were hardware-level), `maintenance`, `QUARANTINED`
or `not attached`.

A board is named by its stand-file id, written on the board (`s31-a`,
`c5-a`). Wherever a board is expected (a command's `--board` or `BOARD`), its
id, its MAC, or its chip when it is the stand file's only board of that chip
selects it; a chip with several boards is refused with their ids.

A board's only reset that does not depend on its USB Serial/JTAG port is its
hub port's power, when its reset ladder includes `power`; the stand has no
UART bridges to the chips' EN and BOOT pins.

Tools reach a board's port only through the stand's commands, which release
RTS before DTR so opening a port never resets the chip, and hold a lease of
the board while they use it; a script or terminal that opens a board's port
itself can reset the chip.
`cargo stand board reset BOARD` resets through the USB Serial/JTAG RTS line
(default), `--via jtag` through OpenOCD and the chip's debug module,
`--via download` by cycling the power of the board's hub port and, as soon as
its USB returns, resetting it into the ROM's download mode (the recovery
ladder's entry for a board whose image switches its USB Serial/JTAG off), or
`--via power` by cycling the
power of the board's hub port (off, then on), and prints the reset
line the ROM reports. A hub port is switched only that way, under the board's
lease: `cargo stand lease` refuses a command that runs `uhubctl`, and the
repository's Claude Code hook refuses `uhubctl` with an action, so no port is
left off. When a lease of a board that resets by power is granted and
when it is released, the arbiter returns the port to its working state: a port
that is off is powered, and a powered port whose board is not attached by its
serial number is cycled; each restoration is a recovery in the board journal.
The grant restores a board its previous holder left without a release. The
stand file's `[[hub]]` lists each hub's switchable ports and the protected
ones a cascaded hub hangs on: a board never hangs on a protected port, and
resets by `power` only on a switchable one. `board check` reports, without resetting, whether the board is
attached, its last flash, maintenance, its reset paths and whether it answers
the peer text protocol. `board console --for DUR [--until TEXT]` prints and
saves the console under `target/hil/console/<mac>/`. `board soak --cycles N
| --for DUR [--via rts,jtag,power] [--batch 10]` resets the board through each
path once per cycle, `--batch` cycles per lease so other owners may use the
board between batches, and stops at the first reset after which the ROM does
not boot from flash, saving the console that follows; the board journal
records the paths, cycles, resets and any failure. `cargo hil peer send
BOARD LINE` sends one peer command and prints the output up to that command's
`@OK` or `@ERR`; it also claims the air shared, since a peer command may
transmit. Every OpenOCD session the stand starts is stopped when it outlives
its timeout (ten minutes to program, one otherwise). A tool that holds a long
interactive session under a board lease, such as the calibration cross-check,
opens the port through the same library functions,
`oer_hil_board::reset::{open_without_reset,
reset_into_application}`. Every serial line of the stand, those consoles,
the flash writer's, the device-under-test session's and the peer consoles,
is opened by one owner, `oer_hil_board::port::Port` (by path, with its
`Settings`: line rate, read timeout, modem lines, busy retry); the link
reads the lines the stand opens for it (`oer_hil_lab::{peer_console,
attach_console}` and the `Dut` console opener). Every reset,
here and in the runner's recoveries, is a rung of the one ladder of
board I/O (`oer_hil_board::reset`).

```console
cargo stand devices                     # the stand file's boards: label, port, last firmware
cargo stand board check esp32c5
cargo stand board reset esp32c5 --via jtag
cargo stand board reset esp32c5 --via power
cargo stand board console esp32c5 --for 30s --until @READY
cargo stand board soak esp32c5 --for 8h --via rts,jtag
cargo hil peer send esp32c5 SYNC
cargo stand lease --board esp32c5 --flashed ieee802154-peer --application build/peer.bin \
    --device esp32c5 -- idf.py -p /dev/ttyACM1 flash
```

## ESP-IDF firmware catalog

Tracked ESP-IDF firmware forms one catalog: every project with a
`firmware.toml` beside its `CMakeLists.txt`, peers in `hil/peers/<project>/`,
vendor references in `firmware/vendor/<chip>/<project>/`; each
image is bundled with its own bootloader and partition table. `hold = "REASON"` in a
manifest refuses every flash of that image, including a run's automatic peer
restore, which then blocks its scenarios with the reason; `firmware list`
shows it. The
manifest names the image as the board journal records it, the target chip and
`pins`, the chip whose `artifacts.toml` pins the ESP-IDF revision and vendor
archives the image builds against: an ESP32-C5 image names `esp32c5`. Builds are reproducible
(`CONFIG_APP_REPRODUCIBLE_BUILD`), so equal sources give an equal digest.

```console
cargo hil firmware list
cargo hil firmware build ieee802154-peer
cargo fw flash ieee802154-peer --device <MAC|PORT>
```

`cargo fw flash IMAGE` builds a catalog image, bundles the build's
application with its own bootloader and partition table (the build's
`flasher_args.json` must place them at the chip's flash map and flash
nothing else) and writes the bundle; a board whose receipt already records
that image's digest is not written again. A board whose chip differs from
the image's target is refused.

The board journal (`board.jsonl` of the arbiter directory) has one writer of
a flash, the flash operation. A flash outside it (a vendor image, a manual
`espflash` or `idf.py flash`) is recorded with `lease --flashed IMAGE`, which
journals it only when the leased command succeeds: name the board with
`--device NAME|MAC` and the application with `--application FILE` or
`--sha256`.

On every grant the arbiter prints the last flashed firmware of every board
(image, commit, application hash, owner) and lists the flashes other owners
made since this owner's previous lease. It only reports this state; it never
erases or restores it. A board's startup artifact (the PHY calibration cache)
is a host file the stand file still names, but no current image uploads or
writes it. `cargo stand queue` lists the
newest recorded upload or write of every such file. The queue, history, journal and jobs live in the arbiter directory of the
user's host cache, so every checkout of the repository shares them. The
arbiter's last exclusion layer is a lock file per board (named by its MAC)
and per fixture resource in `open-esp-radio/leases` of the XDG cache
directory, taken once a lease is granted: a process outside the queue (a
checkout without the arbiter) still fails fast on them, and a granted holder
waits for such a process to finish.

## Host fixtures

`cargo stand fixtures` probes the host fixtures the stand file names:
this host's Wi-Fi radios and Bluetooth adapter and the OpenWrt station fixture
and air observer over SSH. It prints each one's lease key (the claim a run
takes it by), whether it answers, its model and firmware, and each
interface's type, channel (number, frequency and width) and CCA busy share
(busy over active time of the frequency in use, from the radio's survey). A
Bluetooth adapter whose radio switch is soft- or hard-blocked answers no HCI
command and is reported as blocked. The dashboard's Fixtures section shows the
same, refreshed every minute, with the lease holding each fixture.
