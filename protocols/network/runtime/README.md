# oer-protocols-network-runtime

The task that owns a network core, and the sockets applications hold. It binds
no executor: time comes from `oer-portable-time`, and a composition declares the
task that polls `Service::run` and then calls `Service::retire`.

One task owns the core. A socket talks to it through queues, and blocks travel
through them by value. A socket's lanes are its own, so one socket waits for no
other's room. A local producer instead is called by the task itself, in its own
turn: it needs no queue and no block.

`Service::bind` opens a socket in the core and gives the application its end, so a
socket exists only once its service holds the other end. A refused bind changes
nothing and returns the storage.

`run` only drives the service, which keeps every owner, so cancelling it loses
nothing: `run` may start again, or the owner calls `retire`. Dropping a `Service`
is another matter: it is the end of the whole network. Port, pool and brand go, and
waiting sockets and observers end with `SocketClosed` and `Outcome::Retired`. Blocks
in the lanes then stay there, unreachable, and their memory is not reused.

`Socket::send` writes the command into its lane, and that is the acceptance
point: before it a `Refused` with the block, after it one result. Nothing that
takes a block is async, so a cancelled wait never loses one: `credit` and `room`
wait, and `send` and `release` do not.

Each credit has its own result place, shared with the service, and the
`Observer` reads it with no socket: another task may wait for a result while the
socket works, and a result outlives a dropped socket. `finished` waits and may be
cancelled; `take` consumes the observer, so a result is read once and a credit's
next command is out of its reach. Dropping an observer gives up the result, and
the credit frees when the command has ended, whichever comes last.

The service finds results only where the core tells them (`next_end`, see the
core's README, "Work records"): a running command is never looked at to learn
that it did not end. The service keeps, at each command's place in the core,
the right to its result (`CommandId`) and the credit's tag it answers. A
credit's tag and its command's place free at different times, so they need not
be equal. A told end is matched by place and generation; one that is not this
right's command leaves the right where it is. The result goes to its tag's
place once, as before. A producer's works are kept and matched in the same way.

A socket's lanes wake through waker cells, so today a socket and its service
share a hart.

A block comes from the service, which takes a turn. `reserve_ahead` lets a socket hold
up to `RESERVE_AHEAD` ready, so a sender runs a batch on one turn. A socket asks for
what it lacks in one request, of at most `RESERVE_AHEAD` blocks. Taking a block
asks again once half the look-ahead is out, ready or asked for, so the next request
goes while the rest still cover the sender. With the look-ahead of sixteen, a sender
taking the same c blocks between two turns of the service never finds none ready after
its first for c of 1 to 10, 12 and 16. Other bursts, and a larger burst after a smaller
one (sixteen after nine), can still find none and wait for the next request. A
look-ahead of one block asks when it finds none. The service grants
them one by one as blocks come free, each chosen anew among the waiting sockets, so a
larger request gains no place. A block counts toward its group from its grant, not
from the request. Granted blocks stay out of other sockets' reach while they wait, and
a stop or a dropped socket brings them home with the rest of its lanes. A cancelled
`reserve` leaves its request in place for the next one. A close ends what the socket
was still owed, and what it was granted stays its own.

## Publishing

A turn's publishing takes the ready work the core lists first, one frame at a
time, until the port has no room or the turn's eight frames are spent: control
first as listed, then the sockets in turn from the one whose turn it is, one
datagram each, which is what the core's list gives first after each frame it
takes. The network chooses no air: a frame the port takes goes in its queue,
and the port's MAC decides when it goes and charges its attempts (for Wi-Fi,
the station's queues per peer and TID). No await falls between a frame's
choice and the port's answer, so a cancelled `run` leaves no frame unanswered.

Where the core is certainly idle (`NetworkOwner::idle`), nothing is walked and
publishing ends. The core is idle when no ARP reply, echo reply or DHCP
message is owed, no socket's running chain has a head (a producer's declared
works among them), and no neighbor entry owes a request: every source
`demands` lists. Each is read in its own state, with no walk of the running
chains; the neighbor table, last, is a look at each of its entries. Work that
runs but is not ready (a datagram waiting for its neighbor) leaves the core
not idle, and its walk finds nothing. The check comes after the producer's
offers, so an offer of the turn is a running work by then.

| Answer | Owners and state |
| --- | --- |
| accepted | the core commits the work; the next frame goes |
| a final refusal (`Stale`, `Rejected`, `SlotTooShort`) or a decline | the work ends as before; the next frame goes |
| a refusal of the port | the work stays ready; publishing ends for the turn |
| a port fault | publishing ends, the service ends |

Keys are not the network's: the station reports its link up only once
connected, and a key it loses reaches publishing as the port's refusal
(`LinkDown`). A full port or no free buffer is the port's refusal too. The
core's own state (an address, a neighbor, a lease) is read when each frame is
built: a work no longer that work is `Stale`.

The port's notices are settlements of frames it took: a turn takes at most its
budget of them, and a turn that leaves more (`ServiceEvent::NoticesFull`)
turns again after a yield.

Memory is shared and has two guards. A block counts toward the `Group` of the pool it
was taken for until the pool has it back, so moving it between sockets, or dropping
the socket that handed it over, changes nothing: the charge is on the block, in the
pool, for reservations and received data alike. A group has one limit, set where it
is made, which pins no block. When no block is free and a socket under its limit
waits, `MemoryPolicy` may evict: the oldest queued datagram past `evict_after` that
counts toward the group holding the most, and more than the waiter's. It ends
`Outcome::Evicted`, and only datagrams the port has not taken can be evicted, never an
application's blocks. This is pressure, not a deadline: nothing is evicted while none
waits. A block a received datagram cannot have is dropped, and counted as an event.

A block has a way home whichever service holds its pool by then. A `ReturnLane` has a
slot for each block of the pool, so `Returner::give` needs no socket and no live
service of its own, and any number of `Returner` copies share it. `give` and closing
have one order: `Ok` means the lane accepted the block, and a refusal hands it back,
with no third end. The lane closes when its `ReturnEnd` ends, and only then: that end
moves through `Ended::returns` to the next service, and neither that nor a cancelled
`run` closes it. `Ended::close_returns` brings what was accepted to the pool first. If
the end is dropped without that, its blocks stay in the lane, and `Returner::reclaim`
reaches them, as the pool is no more. `Closed` is final.

An earlier service's `LateSocket` can be adopted by the next one with `adopt`, whose
pool it could not reach alone. With no service to come, `Ended::collect` brings home
what has arrived. A charge lifts when the block arrives. Groups belong to the pool and
outlive services: a next service binds the same `Group` values, as a pool keeps
`GROUPS` of them and never frees one.

Dropping a `LateSocket` or a `Service` with commands in a request lane still strands
what those hold: that has no contract yet.

A turn first reads the port's link into the core, then takes commands (four a
lane), the returns, received frames (eight) and the port's notices (up to 64, a
Block Ack's whole ends), then, once it has
read them all, asks its producer for offers, chooses work and publishes it (eight,
over the transmit plan below),
evicts under memory pressure, then hands results and datagrams back. Each part is bounded, so no lane and no port
starves the others, and sockets waiting for blocks take turns. Every exit of a
turn goes through that last part, a fault's too: after a port fault the
turn publishes nothing more, and only hands back what it knows, so at an await
each open work truly was not published.

## Returns and waiting

Returns are what others publish back to the service: lent buffers back with
their port (each `Lent`'s drop counts one), blocks in the return lane, and what
adopted sockets give back. Servicing them is a short owner operation: take what
was published as one batch and give it to the pool and the loans' account, so a
credit frees before the next frame needs it. It runs no command, reads no frame,
chooses no work and builds no frame. Loan accounting stays with the service: an
application only publishes a count. A batch whose blocks came home while a
socket waits for one marks work, and a turn grants it. A turn takes the returns
too, before it reads frames.

After a turn, the service waits, unless the turn left work that no wake
names: a spent budget of frames or publications, notices still untold, a block
evicted, a producer's hold or open budget freed by the turn's own delivery, a
declining fill that kept the producer's flow out of the rest of the turn, or a
refused publication of the address. Then it yields and turns again. A useful
turn is never followed by an empty one only to find nothing.

Each pass of the wait registers every source and reads its readiness from
that registration, so whatever comes after a registration wakes the next pass,
and ordinary work is known apart from returns:

| Source | Ready when the pass registers |
| --- | --- |
| stop, each lane | a message waits; a blocked receive lane has room |
| port (`FrameSource::register`) | its return value: a frame or a notice not yet taken, a link changed since the last registration, room for the frame it last refused |
| producer (`Produce::register`) | with offers wanted, its return value (data `offer` has not handed out); always, a generation the service has not checked yet, or a hold no longer current, which the service reads itself |
| returns | taken at once, as above: ready only if a block came home for a socket that waits for one |
| time | the deadline (the core's, memory pressure's, the producer's) has come |

A registration returns readiness; it does not wake the caller for it. A pass
with nothing ready waits on, whatever woke it, so a return never swallows
another source's wake, and a stray wake costs one pass and no turn. A wait that
is over at its first poll, by a ready source or by a time already due, yields
before the next turn, so one poll holds at most one turn after a wait. A source
that says it is ready for good therefore costs a turn a poll, and it cannot
hold the executor.

## Lending sockets

A socket receives in one mode, chosen at its bind, and paths never switch
under pressure. `bind` copies each datagram into a block of its group.
`bind_lending::<CREDIT>` lends: a datagram in a buffer the port may lend
stays there, under a loan of the socket's credit from the pool's `Loans`,
and the application receives it as a `Lent` from `LendingSocket::receive`.
The lane holds every loan the credit allows, which a const check proves
when the bind compiles, and the account refuses a credit the pool cannot
hold, with the storage returned and nothing changed. A frame the device
copied for its form (`CopyReason::Format`, such as an A-MSDU's MSDU) comes as
a copy in a block of the socket's group; one it copied by pressure is
dropped (`NetEvent::PressureCopy`). A datagram that finds the socket at its
credit is dropped with `NetEvent::NotLent`, and its buffer goes back to the
port at once; a full lent lane is `NetEvent::LentLaneFull`, which the const
check leaves no room for.

Dropping a `Lent` gives its buffer back to the port first and only then
counts its loan back, so a credit frees once its memory has, never at a stop
or a new installation. The service gives the loan to the account at its next
pass, with no turn (see "Returns and waiting"). A stop does not wait for lent buffers: the service ends with loans
out, its `LateSocket` keeps them, and whoever holds the pool by then takes
each late return, through `Ended::collect` or the next service's `adopt`.
The account outlives the core with its pool, so a loan of an earlier
installation counts against the next one's credits until it returns, and
nothing expires.

## Local producer

`Service::with_producer` owns a `Produce` by value, and `bind_local` gives it a
send-only socket: a datagram for its port is dropped before any block is taken,
as `NetEvent::SendOnly`. `()` is the service without one, at no cost. The
producer runs on the service's hart and task; independent and cross-hart clients
keep the channel sockets.

Each turn the service asks `offer` and makes each offer one `Core::declare`, the
acceptance point: a place and no block, the offer's tag kept in it, the work's
one record; the producer keeps a count of open works and nothing else of them.
A refusal is the offer's one answer, `refused`; a declared work has one outcome,
`ended`. A work its port takes ends `Accepted` within its publication, before
the next frame is tried; every other end comes, with its tag, from the core's
ended chain at the next delivery. Once publishing takes the
work and the port gives it a slot, the core writes the Ethernet and IPv4 headers,
the producer's `fill` writes the data in the slot itself, and the core sums the
UDP checksum over it there. Generated data so needs no copy at all.

Progress within one turn is bounded by eight offers and the hold rules. A final
refusal (`TooLong`, `NoRoute`, `Stale`) moves on to the next offer in the same
turn, with no wake. `NoPlace`, `NeighborsFull` and `NoAddress` hold the
producer's offers, the hold belonging to the refused offer: it lasts until one
of the producer's places frees, or the core's `neighbors_changed` counter moves
(an ARP answer or expiry, a resolution given up, every end of work that held a
neighbor), or its `address_version` moves (a lease, its end), or until it is
stale: the source's generation moved, or `current(tag)` is false. A stale hold
gives way to a new offer, which meets `NoPlace` again while the places are taken.
At most eight declared works are open at once. A waiting service registers its
producer for what it can act on: `Interest::Offers` when it would ask at its next
turn, `Interest::Changes` while a hold or that budget stops it, so new data alone
wakes it only when it can declare it; what frees a hold or the budget comes in a
turn of its own.

A source that overwrites what open tags name moves its generation, and the next
turn withdraws each declared work no longer current: it ends `Declined`, with no
slot and no port. Every fill checks `current(tag)` first, so no frame carries a
later generation's data under an earlier work. A declining fill ends its work
`Declined`, moves the next socket's turn past the producer's, and keeps the
producer's flow out of the rest of the turn: one declining fill a turn, and
another socket's work goes. An `Unknown` work is never declared again.

Outcomes reach their clients before the turn ends, on every exit: the delivery
is the turn's last step that can learn a result, after the eviction of memory
pressure too. `retire` ends
each open work `Retired`, tells the producer, returns it in `Ended::producers`
and only then sets the `LocalEnd` flag the image lent. Dropping the service drops
the producer, calls nothing, and its guard sets the flag: every outcome was
delivered before the last await, so every work still open never went out.

## The interface's address

`Service::watch` gives the service a `ConfigurationWatch` to publish the
core's `Configuration` to: at once, then at the end of each turn in which the
core's `configuration_changes` moved. The service's end, `retire` or a drop
of the service, ends the watch for good, as the lease leaves with its core:
reads find no configuration and the waiter wakes, by a flag that borrows no
value cell. A cancelled `run` ends nothing. `changes` counts publications, and `changed` waits
for the next; one task waits at a time. A read meets a publication in progress
only from another hart, and is refused for it, never handed an old value as
new.

The sink lent to `run` hears the core's drops and the service's `ServiceEvent`s:
each turn's beginning, each request it takes by kind, each received frame it takes
in, each socket whose lane still holds requests once the turn took its budget of
them, a full intake, each notice it takes and a full budget of them, each frame the port accepts, how
publishing ended, what was free when the turn ended, and whether the pass after it
yielded or waited. The sink counts; the service keeps nothing and reads no clock for
it. A disabled sink, `()` among them, hears nothing and builds no event.

`Service::run` ends at the first look after a stop, or at once when the port
contradicts itself, and returns the cause. Nothing is published from then on.
`Service::retire` returns `Ended`: the cause, the core's `Retired`, a
`LateSocket` per socket and the producer. Every result the service knows goes to its place, the request lane goes to its socket, and only then
does `stopped` show: what still waits there the service never saw, so those
observers read `Outcome::Retired`, and `Socket::reclaim` gives back their blocks
and all else in the lanes, for the pool's holder. So an end waits for no
application, and a late application loses no result.

Dropping a `Socket` is an abort, without waiting. Its last message hands its
receiving lanes to the service, whose single use leaves it always room. The service
ends the commands it had not seen `Outcome::Closed`, closes the socket in the
core, and gives every block in the lanes to the pool; observers still read their
results. A socket dropped after a stop gives the same to `LateSocket::collect`. A
block in the application's own hands stays there, for the pool's holder.

## Diagnostics

`Service::with_diagnostics` gives the service an image's `Diagnostics`, the
one exception to zero-cost instrumentation (docs/architecture.md,
"Switchable network-core diagnostics"); `()`, the default, is none, and its
points leave no code. Each turn, and each pass of the wait that takes
returns, asks `Diagnostics::on` once. While on, the turn counts into its own
`Diagnosis` on the task's stack, reads the diagnostics' clock at its start
and at each stage's end, and hands the whole count to `Diagnostics::add` at
its end; the core counts its own steps into it through `Count`
(oer-protocols-network-core). Nothing the service or the core
decides reads a count, so the same events decide the same with them on or
off; a switch takes effect at the next turn. These are the counters, and
only these:

| Counter | Counted where |
| --- | --- |
| `turns` | each turn counted, the stages' time is theirs |
| `walked` | each running place a walk of the sockets' running chains visits, ready or not (`Core::demands`); a turn whose core is idle walks nothing |
| `told` | each end an ended chain tells (`Core::next_end`), the sockets' in the delivery and the producer's |
| `return_batches`, `blocks_returned`, `credits_restored` | each return batch that took something (`returns_of`, in a turn or a pass of the wait), the blocks it brought home and the receive loans it gave back to the pool's account |
| `no_credit`, `no_credit_loans`, `no_credit_unreturned` | each `NotLent::NoCredit` refusal in `NetworkOwner::take_frame`, the loans out of the account at the refusal, and the lent buffers back with their port whose loans the service had not yet taken back, read right after the refusal's frame |
| `materialised` | each frame `NetworkOwner::transmit` wrote whole in the port's slot, whatever the port answers |
| `stage_ticks` | the clock between stage ends: `Intake` (link, tick, commands), `Returns` (returns, blocks given), `Receive`, `Notices`, `Publish` (offers, frames), `Settle` (eviction, delivery, the turn's end), and `Waiting`, a pass's return servicing |

Off, a turn reads no clock and updates no counter. What it still costs is
the enable check, one `Diagnostics::on` call per turn and per pass that
takes returns (an observing S31 image's: two atomic loads and a compare;
every other image has `()` and pays nothing), and
at each point above a test of that answer, held in the turn's tally: six
stage ends, one per return batch
and per received frame, and in the core one per running place a walk
visits, per end told, per frame built and per refusal for credit. The
turn's tally stays on the task's stack either way.

## The device port

`DevicePort` is a service's port over a `NetDevice` and the transmit half
that owns the device's transmit buffers (`OwnedTransmit`). It publishes into
those buffers: the transmit half takes a buffer and the room to hand the
frame on together, once, before any fill, and reads the frame's length and
DSCP from the `FrameMeta` its publisher gives, not from the written frame. A
frame the device took carries the transmit half's ticket, and its notices
are those frames' ends: a buffer
the device gave back is free again only when its end is handed on, so no more
frames are unsettled than there are buffers. It shows a lent received frame as
lendable and hands its buffer over when the reader keeps it; a frame the
device copied stays the device's, with its copy's reason. Each
registration asks the transmit half for the memory the device returned, so that
memory comes home whether or not the service sends. It returns whether a
received frame, a link change, a notice or, for a refused frame only, the room
or buffer that frame waits for is there. `give_back` ends the exchange before the port ends: it gives
back every frame the device's queue holds and takes back the memory the device
returned, as the device's queues outlive the port, and leaves no refused frame
waiting, so a next service over the same port starts idle. `serve_returns` is the
return path of a port whose service no longer runs, for a composition that cancels
its network: an owner that outlives the cancelled future keeps the service
(`retire` takes it whenever) and drives the port's returns, frames given back and
memory taken back with a registration for the next of each, until the device's
owner says it is done. Nothing serves the returns of a composition without such an
owner. The S31 application does not cancel its network task: that task is a root
pinned in an executor frame that never returns, and it has no such owner. A change
that cancels it must bring a working return path with it.
