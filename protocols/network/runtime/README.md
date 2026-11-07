# oer-protocols-network-runtime

The task that owns a network core, and the sockets applications hold. It binds
no executor: time comes from `oer-portable-time`, and a composition declares the
task that polls `Service::run`.

One task owns the core. A socket talks to it through queues, and blocks travel
through them by value. A socket's lanes are its own, so one socket waits for no
other's room.

`Service::bind` opens a socket in the core and gives the application its end, so a
socket exists only once its service holds the other end. A refused bind changes
nothing and returns the storage. Dropping a service unrun, or cancelling its task,
drops its pool, so no block has an owner left, and waiting sockets and observers
end: `SocketClosed` and `Outcome::Retired`.

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

A turn takes commands, then received frames, then chooses work and publishes it,
then hands results and datagrams back. Each part is bounded, so no lane and no
port starves the others.

`Service::run` ends at the first look after a stop, or at once when the port
contradicts itself, and returns `Ended`: the cause, the core's `Retired` and a
`LateSocket` per socket. Nothing is published from then on. Every result the
service knows goes to its place, the request lane goes to its socket, and only then
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
