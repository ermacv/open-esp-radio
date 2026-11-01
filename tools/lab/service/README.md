# LAB service

`oer-tool-lab-service` is the one owner of the stand: one process per host
holds its queue, its leases and every access to its hardware. `cargo stand`,
`cargo fw` and `cargo hil` are its clients; they build images, run
scenarios and their own commands, judge results, and ask the service for
every lease and every operation on a board, a hub port or the router. They
build against its protocol and client ([`oer-lab-client`](../client/README.md))
only: the service, the board drivers and the router's SSH stay out of their
builds (`oer-test-layers` checks the lock). The tools are trusted: nothing
keeps a process from opening a port or the router's SSH itself, and nothing
needs to.

## Running it

`cargo stand install` builds `oer-tool-lab-service` from `main` and installs
two systemd user units:

- `oer-lab.socket` holds the stand model's socket,
  `$XDG_RUNTIME_DIR/open-esp-radio/lab.sock`, and hands it to the service: a
  client that connects while the service restarts waits for it.
- `oer-lab.service` runs `oer-tool-lab-service --root CLONE` from the time
  the user logs in.

Started by hand without a socket handed over, the service binds the socket
itself (`$OER_LAB_SOCKET` moves it). A lock in the stand's directory
(`service.lock`) keeps a second instance out. A checkout never replaces the
service: an update builds the new version beside the installed one, asks
the running service to drain (it grants nothing more and answers once no
lease is held and no driver works, a recovery past its deadline included),
and only then switches and restarts the unit. A running
service of another protocol cannot be asked: the update stops with the
installation unchanged.

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

- A lease belongs to the connection that acquired it, with a token. A
  command the client runs under the lease (`cargo stand lease -- CMD`) is
  the client's own child: on a terminal it shares the terminal's
  foreground job, reads it and takes its Ctrl-C itself; otherwise it owns a
  process group. It gets a token in `OER_LAB_LEASE`;
  a client inside it joins the lease for the claims it names, which the
  token must allow, and gets a token of its own allowing those claims only:
  each level of delegation narrows. An ended lease's tokens join nothing.
- An operation runs on the service's drivers and streams its output to its
  client and its client's input to it. Output waiting for a slow reader is
  bounded on both sides (1 MiB): what does not fit is dropped and counted,
  never waited for, so neither the hardware work nor the replies behind it
  wait for a reader. Input flows by credit: a client sends at most 64
  frames ahead of what the operation took; input beyond the window fails
  the operation with that reason.

## Life cycle

A lease is granted once the scheduler serves it; then the drivers prepare
its resources (the real stand's drivers have nothing to prepare), and only
once the preparation left the lease does its client hear the grant. A
resource that cannot be prepared ends the lease, which recovers it, and
the client hears why.

A lease ends when its client releases it, when its connection is lost, at
the hard limit, on a person's preemption, or for waiting stand maintenance;
a revoked lease's client hears why. Its operations are told to stop first,
and the lease ends only once each returned: a driver ends the hardware work
and the programs it started (a flash, OpenOCD) before its operation
returns. Then every stateful resource it held goes to its driver's
recovery, which brings it to its end state actively, whatever the lease
did, and only a confirmed end state makes it ready again. A client that
releases its lease hears the end once every recovery ended, with each
resource left unavailable and why; `cargo hil`, `cargo fw` and `cargo stand
lease` fail on such an end. A recovery has a deadline (180 s); one past it
leaves the resource unavailable, untouched until the driver returns. A grant
the service could not persist is refused. A restart ends every lease still
held as abandoned and recovers its resources before granting them again.

## Drivers

[`stand`](src/stand.rs) drives the real stand through `oer-lab-board` and
`oer-lab-router`. A board's radio works only on its image's command, so a
board has no end state to prove, and an operation on a board (a flash, a
reset, a console, a power cycle) needs only the board: the air is claimed
by the run that commands a radio (a HIL scenario declares it). A router
operation turns the router's own radio on and needs a lease that holds
the air, shared at least.

- **Boards.** Each board has one owner in the service: its operations take
  turns in the order they asked (two operations of one lease never drive one
  board at once). After a lease, a board that resets over its USB
  Serial/JTAG (`usb-jtag-rts`) is reset into its image, which stops what
  the lease left running; it is ready again whatever the reset did, and a
  failed reset is only logged. Any other board (an ESP32-C5, reset only by
  JTAG and power) is not reset: it is ready at once. A release that
  answers without an unavailable resource does not confirm a reset.

  Operations: write and start an image (journaled), reset (through a path,
  or the first reset the stand file lists), the console as a duplex stream,
  a watched power cycle and its proof of power loss, where a board is
  attached and what it carries, and the detection of every attached board's
  chip under a lease of the whole stand (`cargo stand init`).
- **Hub ports.** A port is switched off and on only under a lease of
  `hub-port:LOCATION:PORT`. The service runs `uhubctl` for every report.
- **The router.** It is a resource when the stand file names a `[router]`.
  Its end state: the stand's AP section disabled with the rest of its
  configuration kept, and its radio quiet: netifd's status (`ubus call
  network.wireless status`) shows the radio's configuration applied, not
  pending, and `iw dev` lists no interface of any type on the radio's PHY
  (`iwinfo nl80211 phyname`). Both views must answer; one that fails
  leaves the router unavailable, never parked. A run's profile enables the
  section again. Its operations are the logic of
  `oer-lab-router`: the AP profile on a channel the stand file allows (its
  isolated network forwarding UDP and TCP with one host when the run names
  it), the station, DHCP and ping views, the station network's DHCP server
  stopped or serving, the WPA2 checks, a deauthentication, the
  routed network's check and the echo and departure observers. The service
  probes the equipment it reaches over SSH (the router, the air observer)
  under a lease that holds the router; the host's own radios are probed by
  `cargo stand fixtures` itself and run by their fixture launchers.

## Checks

```console
cargo test -p oer-tool-lab-service
```

The tests run the service in the test process with fake drivers and talk to
it over socket pairs: protocol versions, conflicting and independent leases,
a client lost during hardware work, a failed and a late recovery, a restart
with a lease held, stale confirmations, a slow reader and an unread
operation, revocations, joined clients and the narrowing of their tokens,
input by credit and beyond the window, a release that waits for its
recoveries, a drain that waits for them, a grant that cannot be prepared,
the first operation after a grant and an unrecordable grant; the turns of
operations on one board, a board leased and used while the router is
unreachable, the operations that need the air, and the router's PHY judged
by both views.
