# oer-protocols-network-runtime

The task that owns a network core, and the sockets applications hold. It binds
no executor: time comes from `oer-portable-time`, and a composition declares the
task that polls `Service::run` and then calls `Service::retire`.

One task owns the core. A socket talks to it through queues, and blocks travel
through them by value. A socket's lanes are its own, so one socket waits for no
other's room.

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

A block always has a way home, whichever service holds its pool by then. A `Returner`
takes it there through a `ReturnLane`, which holds a pool's every block and goes from
service to service with `Ended::returns`, so a block needs no socket and no live
service of its own. An earlier service's `LateSocket` can be adopted by the next one
with `adopt`, whose pool it could not reach alone. With no service to come,
`Ended::collect` brings home what has arrived. A charge lifts when the block arrives.

A turn takes commands, received frames and the port's notices, then, once it has
read them all, chooses work and publishes it, then hands results and datagrams back. Each part is bounded, so no lane and no
port starves the others, and sockets waiting for blocks take turns.

`Service::run` ends at the first look after a stop, or at once when the port
contradicts itself, and returns the cause. Nothing is published from then on.
`Service::retire` returns `Ended`: the cause, the core's `Retired` and a
`LateSocket` per socket. Every result the service knows goes to its place, the request lane goes to its socket, and only then
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
