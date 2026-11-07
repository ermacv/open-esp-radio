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
`Outcome::Unknown`, which is not repeated.

`tick` comes first at every time `poll_deadline` names and at any later `now`
before work is asked for: an address past its life is no work until then.

`scope`, or `scope_async` for a task, gives a pool and a core a brand of their own. Blocks are values lent
from that pool, and handles and blocks of another scope do not compile, so a
block always has its place. A socket has `CREDITS` command places and a place of its own for its
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

`retire` ends a core: its blocks return to the pool and the brand comes back for
the next core. Blocks applications hold stay out (`Pool::outstanding`) and can
return later; old handles find `CommandState::Gone`, as serials go on across
cores.
