# oer-protocols-network-runtime

The task that owns a network core, and the sockets applications hold. It binds
no executor: time comes from `oer-portable-time`, and a composition declares the
task that polls `Service::run`.

One task owns the core. A socket talks to it through queues, and blocks travel
through them by value. A socket's lanes are its own, so one socket waits for no
other's room.

`Socket::send` writes the command into its lane, and that is the acceptance
point: before it a `Refused` with the block, after it one result, which the
`Observer` holds the right to. Nothing that takes a block is async, so a cancelled
wait never loses one: `credit` and `room` wait, and `send` and `release` do not.
Waiting for a result may be cancelled and begun again; dropping an `Observer`
gives up the result, and its credit frees when the command ends.

A socket's lanes wake through waker cells, so today a socket and its service
share a hart.

A turn takes commands, then received frames, then chooses work and publishes it,
then hands results and datagrams back. Each part is bounded, so no lane and no
port starves the others.

`Service::run` ends at the first look after a stop was asked: nothing is published
from then on, and it returns the core's `Retired`. Every result the service knows
goes to its socket's answer lane, which holds all a socket can be owed, and the
request lane follows it. What still waits there the service never saw: its
datagrams end `Outcome::Retired` at the socket, and `Socket::reclaim` gives back
their blocks and all else its lanes hold, for the pool's holder. So a stop waits for
no application, and a late application loses no result.
