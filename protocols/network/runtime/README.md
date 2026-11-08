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

A service's `Egress` policy chooses what goes out, among all ready work and before
any frame exists, and keeps its own account: `Direct` is the first ready work and no
account. No await falls between a choice and its answer from the port, so a
cancelled `run` leaves no reservation unanswered. A policy that cannot go on ends the
service with `EndCause::Egress`, and `Ended` returns the policy with its account.

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

A turn takes commands, received frames and the port's notices, then, once it has
read them all, asks its producer for offers, chooses work and publishes it,
evicts under memory pressure, then hands results and datagrams back. Each part is bounded, so no lane and no port
starves the others, and sockets waiting for blocks take turns. Every exit of a
turn goes through that last part, a fault's too: after a policy or port fault the
turn publishes nothing more, and only hands back what it knows, so at an await
each open work truly was not published.

## Local producer

`Service::with_producer` owns a `Produce` by value, and `bind_local` gives it a
send-only socket: a datagram for its port is dropped before any block is taken,
as `NetEvent::SendOnly`. `()` is the service without one, at no cost. The
producer runs on the service's hart and task; independent and cross-hart clients
keep the channel sockets.

Each turn the service asks `offer` and makes each offer one `Core::declare`, the
acceptance point: a place and no block. A refusal is the offer's one answer,
`refused`; a declared work has one outcome, `ended`. Once the policy chooses the
work and the port gives it a slot, the core writes the Ethernet and IPv4 headers,
the producer's `fill` writes the data in the slot itself, and the core sums the
UDP checksum over it there. Generated data so needs no copy at all.

Progress within one turn is bounded by eight offers and the hold rules. A final
refusal (`TooLong`, `NoRoute`, `Stale`) moves on to the next offer in the same
turn, with no wake. `NoPlace` and `NeighborsFull` hold the producer's offers, the
hold belonging to the refused offer: it lasts until one of the producer's places
frees, or the core's `neighbors_changed` counter moves (an ARP answer or expiry,
a resolution given up, every end of work that held a neighbor), or until it is
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

The sink lent to `run` hears the core's drops and the service's `ServiceEvent`s:
each request a turn takes by kind, each socket whose lane still holds requests once
the turn took its budget of them, a full intake, each frame the port accepts, how
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
