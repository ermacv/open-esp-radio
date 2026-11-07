# oer-protocols-network-core

A sans-IO network core with one owner: IPv4, ARP and UDP over one Ethernet
interface. It takes frames, commands and the current time as values and gives
work back as values. It awaits nothing, holds no waker and places no memory.

Work is chosen before it exists. `demands` lists what is ready, with its peer,
flow and priority, without building a frame; a `Grant` chooses one piece, named
by a serial that never returns, so an old grant cannot send later work.
`transmit` takes the port and does the rest in one call: the port gives its
room, the frame is written there, and the port's answer is committed before the
call returns. No frame leaves unknown to the core, and the core has no state for
work at the port. A datagram's outcome means accepted by the port, not delivered:
until then its block stays with the core, and a refusal moves nothing.

`scope` gives a pool and one core a brand of their own. Blocks are values lent
from that pool, and handles and blocks of another scope do not compile, so a
block always has its place. A socket has `CREDITS` command places and a place of its own for its
close. A place frees when its command finished and its result was read or its
observer dropped; a dropped observer alone frees nothing, so a late end never
reaches a later command.

A datagram waiting for its neighbor holds a block, never a port slot. Its next
hop's station address is read when the frame is built, so a change in the
neighbor table reaches a datagram that waits; a neighbor that expired is asked
again, and one that does not answer ends the datagram with
`Outcome::Unreachable`.
