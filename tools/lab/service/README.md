# LAB service

`oer-tool-lab-service` is the one owner of the stand: one process per host holds
its queue, its leases and every access to its hardware. `cargo stand`,
`cargo fw` and `cargo hil` are its clients; they build images, run scenarios
and judge results, and ask the service for every lease and every operation
on a board, a hub port or the router. They build against its protocol and
client ([`oer-lab-client`](../client/README.md)) only: the service, the
board drivers and the router's SSH stay out of their builds
(`oer-test-layers` checks the lock).

## Running it

`cargo stand install` builds `oer-tool-lab-service` from `main` in its own clone
and installs two systemd user units:

- `oer-lab.socket` holds the stand model's socket,
  `$XDG_RUNTIME_DIR/open-esp-radio/lab.sock`, and hands it to the service: a
  client that connects while the service restarts waits for it.
- `oer-lab.service` runs `oer-tool-lab-service --root CLONE` from the time the
  user logs in, so every board is parked before anyone asks. It delegates its
  control group to the service (`Delegate=yes`): the service moves itself
  into the leaf `service` and runs each command in a group of its own below
  `commands`. Stopping or restarting the unit ends every process of the
  group, also ones that left their session, before the service starts
  again.

Started by hand without a socket handed over, the service binds the socket
itself (`$OER_LAB_SOCKET` moves it); it still needs a control group it may
write. A lock in the stand's directory (`service.lock`) keeps a second
instance out. A checkout never replaces the service: an update by `cargo
stand install` first asks the running service to drain (it grants nothing
more and answers once no lease is held), then restarts the unit on the new
binary. A running service of another protocol cannot be asked: the update
stops and says how to replace it by hand. The first version runs as the
stand's user.

The service's state lives in `$XDG_STATE_HOME/open-esp-radio/arbiter`
([`oer-lab-arbiter`](../arbiter/README.md)): the restrictions on the stand
must survive what a cache may lose. Before the stand file exists (`cargo
stand init` writes it) the stand has no resource; `Request::Reload` reads it
once it does.

## Protocol

One Unix stream socket ([`protocol`](../client/src/protocol.rs)). Every
frame is a big-endian `u32` length, one kind byte and the payload: a JSON
request or reply, or the bytes of an operation's stream after its `u64`
identity. Frames are at most 1 MiB. The first request is `hello` with the
client's protocol version; another version is refused with the reason and
the connection ends.

- A lease belongs to the connection that acquired it. A command the service
  runs under a lease gets the lease's token in `OER_LAB_LEASE`; a client
  inside it joins that lease for the claims it names, which the lease must
  cover, and its operations act with those claims only.
- An operation runs on the service's drivers and streams its output to its
  client and its client's input to it. Output waiting for a slow reader is
  bounded (1 MiB per connection): what does not fit is dropped and reported
  to the client, never waited for, so a slow reader never holds hardware
  work. Input flows by credit: a client sends at most 64 frames ahead of
  what the operation took, and each frame taken returns a credit; input
  beyond the window fails the operation with that reason.

## Life cycle

A lease is granted once the scheduler serves it; then the drivers prepare
its resources from their end states (a board parked with its power off is
powered on and must return on USB), and only then does its client hear the
grant. A resource that cannot be prepared is left unavailable and the lease
is refused.

A lease ends when its client releases it, when its connection is lost, at
the hard limit, on a person's preemption, or for waiting stand maintenance;
a revoked lease's client hears why. Its operations are told to stop first,
and the lease ends only once each returned: a lost client releases nothing
while its hardware work still runs. Then every stateful resource it held
goes to its driver's recovery, and only a confirmed end state makes it ready
again. A client that releases its lease hears the end once every recovery
ended, with each resource left unavailable and why; `cargo hil`, `cargo fw`
and `cargo stand lease` fail on such an end. A recovery has a deadline
(180 s); one past it leaves the resource unavailable, untouched until the
driver returns.

Before an operation starts, the service records in its persisted state
every resource the operation may leave in a state its recovery cannot
confirm (a command under a lease that holds the router may change the
router over its SSH): the end of the lease leaves it unavailable, also when
a restart ends the lease. An operation that cannot confirm that what it
started ended (a command group that does not empty) leaves every resource
of its lease unavailable. A grant or a mark the service could not persist
is refused. A restart ends every lease still held as abandoned and recovers
its resources before granting them again.

## Drivers

[`stand`](src/stand.rs) drives the real stand through `oer-lab-board`:

- **Boards.** Each board has one owner in the service: its operations take
  turns in the order they asked (two operations of one lease never drive one
  board at once), and it keeps the board's port while the board is parked.
  A board's end state follows the resets its stand file allows:
  - A board that resets over its USB Serial/JTAG (`usb-jtag-rts`) rests in
    its ROM's download mode: a reset into it, then a reset line the ROM
    printed after that reset, naming a download boot that serves the USB
    Serial/JTAG (`boot:0x.. (DOWNLOAD(USB/UART0))`). The service keeps its
    port open: closing a USB Serial/JTAG port can move its reset lines. A
    console that starts the image opens the port anew, since a reset from
    download re-enumerates USB.
  - A board that resets only by JTAG and power (an ESP32-C5: its USB reset
    enters a download that no longer serves USB, `docs/hardware-errata.md`)
    rests with its power off: its switchable hub port, found under it on the
    host and allowed by the stand's hub policy, is recorded in the stand's
    directory (`powered-off.json`), switched off, must report off and the
    board leave USB; its chip needs no profile of the board operations for
    this. Its next lease powers it on: the port must report power and the
    board's port return, named by udev, before the client hears the grant.
  - A board without either, or one whose end state is not confirmed, is
    unavailable until a person returns it, with unknown emission unless its
    ROM was seen in a download mode.

  Operations: write and start an image (journaled), reset, the steps of the
  reset ladder, the console as a duplex stream, a watched power cycle and
  its proof of power loss, where a board is attached and what it carries,
  and the detection of every attached board's chip under a lease of the
  whole stand (`cargo stand init`).
- **Hub ports.** A port is switched off and on only under a lease of
  `hub-port:LOCATION:PORT`. The service runs `uhubctl` for every report.
- **The router.** It is a resource when the stand file names a `[router]`.
  Its end state is an access point that answers: it answers over SSH and its
  AP interface runs as an access point; otherwise it is unavailable with
  unknown emission, which closes the air. Its operations are the logic of
  `oer-lab-router`: the AP profile on a channel the stand file allows, the
  station, DHCP and ping views, the WPA2 checks, a deauthentication, the
  routed network's check and the echo and departure observers. The service
  probes the equipment it reaches over SSH (the router, the air observer)
  under a lease that holds the router; the host's own radios are the
  fixtures' (`oer-lab-fixture`).
- **Commands.** `cargo stand lease -- COMMAND` runs COMMAND as the service's
  child with the client's arguments, directory and environment; its output
  and input pass through the operation's stream, on a pseudo-terminal that
  is its controlling terminal when the client runs on a terminal. Each
  command runs in a control group of its own, which it enters before
  `exec`. A stop first closes the group's admission (a spawn under way
  finishes inside it), asks the command's process group to end, then kills
  the whole group (`cgroup.kill`) and waits until it is empty (`populated
  0`) and its output read. A command marked to run to its end (`fixture
  install`'s installer under sudo) is never stopped: its lease waits for it.

## Checks

```console
cargo test -p oer-tool-lab-service
```

The tests run the service in the test process with fake drivers and talk to
it over socket pairs: protocol versions, conflicting and independent leases,
a client lost during hardware work, a failed and a late recovery, a restart
with a lease held, stale confirmations, a slow reader, revocations, joined
commands and their claims, marks before and across a restart, input by
credit and beyond the window, a release that waits for its recoveries, a
grant that cannot be prepared and an unrecordable grant; and the turns of
operations on one board. The control groups, terminals and sudo are checked
on the stand's host.
