# oer-protocols-network-core

A sans-IO network core with one owner: IPv4, ARP and UDP over one Ethernet
interface. It takes frames, commands and the current time as values and gives
work back as values. It awaits nothing, holds no waker and places no memory.

The core takes values and gives values. Work is chosen before it exists:
`demands` lists what is ready, with its peer, flow and priority, without building
a frame; a `Grant` chooses one piece, named by a serial that never returns and
by the station address it was chosen for, so an old grant sends neither later
work nor work to another address. `NetworkOwner` alone holds the core, the pool
and the port, and its `transmit` builds the frame, lets the port write and
accept it, and commits the answer in one call. No frame leaves unknown to the
core, and the core has no state for work at the port. A datagram's outcome means
accepted by the port, not delivered: until then its block stays with the core,
and a refusal moves nothing. The owner keeps its own record of what the frame's
fill did and reads the port's answer against it. A fill that fails comes back as
the fill's own error, never as a port refusal, and ends a datagram
`Outcome::Rejected`, or a producer's declined work `Outcome::Declined`. A port whose answer contradicts that record stops all
publishing until the core retires: `Ok` after a failed or missing fill ends the
work `Outcome::Unknown`, not repeated, whatever the fill's error, and a refusal of
its own after a failed fill ends the work by the fill's cause. Every end at
publication, a frame taken or refused for good, passes the turn to the next
socket.

A local producer's work is declared, not sent: `declare` takes a command place
with `send`'s refusals and no block, and `open_local` gives the producer a
send-only socket, whose port's datagrams are dropped before any block
(`NetEvent::SendOnly`). Its frame's headers go into the port's slot first, then
`transmit`'s `produce` fills the data there, and the UDP checksum, summed over
that data in the slot, goes last. A producer that declines (`Declined`) ends the
work `Outcome::Declined`, which also passes the turn to the next socket, and a
`withdraw` ends it so before any port saw it. A declared work is never queued
for eviction: it holds no block. `neighbors_changed` counts whatever may have
freed a neighbor entry for a new send: an ARP answer, an entry asked again past
its life, a resolution given up or refused, and every end of work that held a
neighbor, so a waiter for `NeighborsFull` keeps the value it saw.

`tick` comes first at every time `poll_deadline` names and at any later `now`
before work is asked for: an address past its life is no work until then.

Any type can be a brand. `claim` lets it claim once, giving one pool and the right
to one core, and this crate holds that record, so no code outside can claim a brand
twice. An image has room for 8 brands, and `test-support` for a test binary's. Blocks are values lent from that pool, and
they and every handle carry the brand, invariantly. A brand is a type rather than a
lifetime, so `'static` tasks and their queues can name a block, and another
network's value does not compile. A block
always has its place in its pool. A socket has `CREDITS` command places and a place of its own for its
close. A place frees when its command finished and its result was read or its
observer dropped; a dropped observer alone frees nothing, so a late end never
reaches a later command.

A datagram waiting for its neighbor holds a block, never a port slot. An
accepted datagram's neighbor keeps its entry: no datagram asks another's neighbor
away, and past the table's size `send` refuses with `NeighborsFull`. Its station
address is read when the frame is built, so a change reaches a datagram that
waits, and a grant chosen for the old address goes stale. A neighbor that expired
is asked again, and one that does not answer ends the datagram with
`Outcome::Unreachable`.

`retire` ends a core and hands on what it held, each to its own owner: blocks to
the pool, the brand to the next core, the port back, and every unread result to
`RetiredResults`, so a confirmed result does not depend on having been read in
time. A datagram still running ends `Outcome::Retired`. Blocks applications hold
stay out (`Pool::outstanding`) and can return later. Serials go on across cores,
so an old handle finds `CommandState::Gone` at the next core and its result in
`RetiredResults`.
