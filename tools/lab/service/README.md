# LAB service

`oer-lab-service` is the one owner of the stand: one process per host holds
its queue, its leases and every access to its hardware. `cargo stand`,
`cargo fw` and `cargo hil` are its clients; they build images, run scenarios
and judge results, and ask the service for every lease and every operation
on a board, a hub port or the router.

## Running it

`cargo stand install` builds `oer-tool-lab-cli` from `main` in its own clone
and installs it with the systemd user unit `oer-lab.service`, which runs
`oer-tool-lab-cli __serve` from the time the user logs in. The unit keeps
every process a hardware command starts in its control group, also ones that
leave its process group or session; stopping or restarting the unit ends
them all before the service starts again. The service binds the stand
model's socket, `$XDG_RUNTIME_DIR/open-esp-radio/lab.sock` (or
`$OER_LAB_SOCKET`); a lock in the stand's directory (`service.lock`) keeps a
second instance out. A checkout never replaces the service: an update by
`cargo stand install` first asks the running service to drain (it grants
nothing more and answers once no lease is held), then restarts the unit on
the new binary. The first version runs as the stand's user.

## Protocol

One Unix stream socket ([`protocol`](src/protocol.rs)). Every frame is a
big-endian `u32` length, one kind byte and the payload: a JSON request or
reply, or the bytes of an operation's stream after its `u64` identity.
Frames are at most 1 MiB. The first request is `hello` with the client's
protocol version; another version is refused with the reason and the
connection ends.

- A lease belongs to the connection that acquired it. A command the service
  runs under a lease gets the lease's token in `OER_LAB_LEASE`; a client
  inside it joins that lease for the claims it covers instead of queueing.
- An operation runs on the service's drivers and streams its output to its
  client and its client's input to it. Output waiting for a slow reader is
  bounded (1 MiB per connection): what does not fit is dropped and reported
  to the client, never waited for, so a slow reader never holds hardware
  work. The client's reader slows the connection rather than buffer without
  bound.

## Life cycle

A lease ends when its client releases it, when its connection is lost, at
the hard limit, on a person's preemption, or for waiting stand maintenance;
a revoked lease's client hears why. Its operations are told to stop first,
and the lease ends only once each returned: a lost client releases nothing
while its hardware work still runs. Then every stateful resource it held
goes to its driver's recovery, and only a confirmed end state makes it ready
again ([`oer-lab-arbiter`](../arbiter/README.md)). A recovery has a deadline
(180 s); one past it leaves the resource unavailable, and its resource stays
untouched until the driver returns. A grant the service could not persist is
refused and its resources recovered. A restart ends every lease still held
as abandoned and recovers its resources before granting them again.

## Drivers

[`stand`](src/stand.rs) drives the real stand through `oer-lab-board`:

- **Boards.** A board's end state is its ROM's download mode: a reset into
  it, then a reset line the ROM printed after that reset, naming a download
  boot that serves the USB Serial/JTAG (`boot:0x.. (DOWNLOAD(USB/UART0))`).
  A board without that line after a power cycle into download (where its hub
  port switches) is unavailable. The service keeps a parked board's port
  open: closing a USB Serial/JTAG port can move its reset lines. Operations:
  write and start an image (journaled), reset, the steps of the reset
  ladder, the console as a duplex stream, a watched power cycle and its
  proof of power loss.
- **Hub ports.** A port is switched off and on only under a lease of
  `hub-port:LOCATION:PORT`.
- **The router.** It is a resource when the stand file names a station
  fixture. The service probes the stand's SSH fixtures under a lease that
  holds the router. A command run under a lease that holds the router may
  change it beyond what the service can confirm: the router is then
  unavailable until a person returns it.
- **Commands.** `cargo stand lease -- COMMAND` runs COMMAND as the service's
  child with the client's arguments, directory and environment; its output
  and input pass through the operation's stream.

## Checks

```console
cargo test -p oer-lab-service
```

The tests run the service in the test process with fake drivers and talk to
it over socket pairs: protocol versions, conflicting and independent leases,
a client lost during hardware work, a failed and a late recovery, a restart
with a lease held, stale confirmations, a slow reader, revocations, joined
commands and an unrecordable grant.
