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
and a refusal moves nothing. A port that contradicts itself ends the work with
`Outcome::Unknown`, not repeated, and stops all publishing until the core retires.

`tick` comes first at every time `poll_deadline` names and at any later `now`
before work is asked for: an address past its life is no work until then.

Any type can be a brand. `claim` lets it claim once, giving one pool and the right
to one core, and this crate holds that record, so no code outside can claim a brand
twice. An image has room for 64 brands. Blocks are values lent from that pool, and
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
