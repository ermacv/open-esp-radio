# oer-protocols-network-core

A sans-IO network core with one owner: IPv4, ARP and UDP over one Ethernet
interface. It takes frames, commands and the current time as values and gives
work back as values. It awaits nothing, holds no waker and places no memory.

Work is chosen before it exists. `poll_demand` says what is ready without
building a frame; a `Grant` chooses it; `materialize` writes it into the slot the
port gave. The result borrows the core, so the port's `accepted` or `refused`
comes before the next received frame, by type, and the core has no state for
work at the port. A datagram's outcome means accepted by the port, not
delivered: until then its block stays with the core, and a refusal moves
nothing.

Blocks are values lent from a `Pool` the composition owns, so a pool outlives a
core. A socket has `CREDITS` command places and a place of its own for its
close. A place frees when its command finished and its result was read or its
observer dropped; a dropped observer alone frees nothing, so a late end never
reaches a later command.

A datagram waiting for its neighbor holds a block, never a port slot. A neighbor
that does not answer ends the datagram with `Outcome::Unreachable`.
