# Stand scheduling

`oer-lab-arbiter` is the scheduling library of the LAB service
(`oer-tool-lab-service`), the one owner of the stand's leases and hardware. It
decides who is granted what and when a resource may be granted again; it
runs no hardware and reads no clock: the service passes the time, runs the
recoveries the scheduler returns and reports each driver's confirmation.
`cargo stand --help` lists the commands that reach it through the service.

## Requests and grants

- Every lease is charged to one validated owner name (`Owner`). A checkout
  registers its owner once with `cargo stand owner set NAME` (`owners.json`,
  which commands read to name their owner); a request from an unregistered
  checkout without an explicit owner is refused. `cargo stand owner merge
  OLD NEW` moves an old owner name's balance and history to a registered
  one.
- A request claims resources, each exclusively or shared: `board:<MAC>`,
  `router`, fixture keys, `hub-port:<LOCATION>:<PORT>`, `air`, frequency
  ranges (`claims::spectrum`), or `stand`, which conflicts with everything.
  Holders whose claims do not conflict hold leases at once.
- A request is granted once no holder and no waiting request served before
  it conflicts with it (`queue`): a maintenance request first, then the one
  whose owner has the higher balance, the earlier one on a tie (`balance`).
  Balances decay with a two-hour half-life, every held lease charges its
  owner the elapsed time, an owner whose request waits behind another
  owner's is credited it, the mean over the owners active in the last day is
  subtracted, and they are clamped to an hour either way.
- A request is also granted only when every stateful resource it holds (a
  board, the router) is ready, and, when it uses the air (the air, a
  frequency range or the whole stand), what every other stateful resource
  of the stand may put on the air is known ([`readiness`](src/readiness.rs)).
- The service ends a lease at `HARD_LIMIT` (one hour), on `cargo stand
  preempt ID --reason TEXT`, and for a waiting maintenance request that
  conflicts with it; divisible work held for `MIN_SLICE` (ten minutes) is
  asked to yield to a waiting owner with a higher balance. A preempted lease
  is charged only up to its preemption, and its history record names who
  preempted it and why.

## Readiness

Each stateful resource is `Ready`, `InUse` by a lease, `Recovering`, or
`Unavailable` with a reason. A grant puts it in use; the end of the lease
starts its recovery, and only its driver's confirmation of an end state its
kind declares makes it ready again. Every grant and recovery starts a new
generation: a confirmation of an older generation changes nothing.

Whether a resource may be granted is apart from what it may put on the air
(`Emission`): a ready resource rests in an end state its driver confirmed
(a board that cannot radiate, the router's known access point); one in use
radiates under its lease's air claims; one recovering, or unavailable
without a confirmation that it cannot radiate, may radiate unknown to the
stand and closes the air to every other request that uses it.

An in-use resource may carry a mark: an operation of its lease may have
left it in a state its recovery cannot confirm. The end of the lease then
leaves it unavailable without a recovery, also when a restart ends the
lease. On a restart the service ends every lease still held as abandoned
and recovers its resources, the ready ones and any unfinished recovery too,
before it grants them again. A person returns an unavailable resource with
`cargo stand devices release`.

A board under maintenance (`cargo stand devices maintenance`) serves only
the owner who took it out; other requests wait, a tool that acts on one
board is told at once. Maintenance never stops a lease already held.

## Records

The directory is `open-esp-radio/arbiter` in the XDG state directory
(`$XDG_STATE_HOME`, else `~/.local/state`), or `OER_STAND_ARBITER_DIR`
([`Arbiter`](src/store.rs)): it holds restrictions on the stand that must
not be lost with a cache. Only the service writes it, except the owner
registry. `cargo stand install` imports once what the file-locked arbiter
kept in the cache directory before the service ([`import`](src/import.rs)):
balances, maintenance, and each quarantined board as unavailable with
unknown emission; it moves the history, the journal and the registry, and
renames the old directory to `arbiter.before-lab-service`. A record it
cannot read, or a lease still held there, refuses the import.

| File | Content |
| --- | --- |
| `service.json` | Schema 4: holders, balances, readiness (with marks) and maintenance. Another schema is refused, never converted |
| `powered-off.json` | The service's driver: each board parked with its power off and the hub port it was switched off at |
| `service.lock` | Held by the running service: a second instance is refused |
| `history.jsonl` | Completed leases: owner, work, scenarios, duration, the time charged, the owner's balance at release, the grant's reason and `released`, `yielded-to-balance`, `hard-limit`, `preempted-on-request` or `abandoned` |
| `board.jsonl` | Flashes, startup-artifact uploads and writes, automatic recoveries, returns of unavailable boards and soaks, with owner, checkout and board MAC |
| `owners.json` | Schema 1: checkout directory to registered owner; the innermost registered directory decides |

The board journal reports board changes but never restores board state. Its
only writer of a flash is `journal::Journal::record_flash`, called by the
service's flash and for `cargo stand lease --flashed`; `Journal::record`
records every other change and refuses a flash. The board image store's
receipts decide what a board currently carries. The JSON-lines files keep
their newest 2000 records once they exceed 1 MiB; readers skip lines they
cannot parse.

## Limitations

The stand is local to one host. Duration estimates, used only for expected
starts, count only released leases of the identical command line or of
single scenarios. Claims are cooperative: a command that claims one board
and uses another is not detected.
