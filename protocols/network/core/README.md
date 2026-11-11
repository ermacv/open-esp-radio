# oer-protocols-network-core

A sans-IO network core with one owner: IPv4, ARP, UDP, a DHCPv4 client and
ICMP echo replies over one Ethernet interface. It takes frames, commands and the current time as values and gives
work back as values. It awaits nothing, holds no waker and places no memory.

The core takes values and gives values. Work is chosen before it exists:
`demands` lists what is ready, with its peer, flow and priority, without building
a frame. `idle` says, with no walk of the running chains, that the list is
certainly empty: no ARP reply, echo reply or DHCP message owed, no running chain
with a head, and no neighbor entry owing a request (a look at each of the
table's entries); `false` says only that work may be ready. A `Grant` chooses one piece, named by a serial that never returns and
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

## Materialising the chosen work

A frame is built only for the work chosen, in the port's slot, in one call
(`NetworkOwner::transmit`). The grant carries what its demand already
checked, with the conditions under which it holds, so the frame does not
check it again while they hold:

| Carried | Holds while | Otherwise |
| --- | --- | --- |
| the next hop's station address (datagrams, echo replies) | no known neighbor entry was written or replaced since (the table counts each such change; the count never comes back to a value it had, and once it reaches its end nothing found holds) and the entry's expiry is still ahead | the table is asked again; another station, or none, is `Unsent::Stale` |
| the work's place and serial, the station address it was chosen for | the place still runs that serial | `Unsent::Stale` |

What the work is (its socket, its remote, its data) is read from its place
at the frame, and the address it goes from from the interface at that time,
as before; the DHCP client's messages and ARP frames are checked anew, as
control work is rare. A frame's headers are written into the slot first.
A stored datagram's data is then copied from its block into the slot and
summed with it (`Checksum::copied`): the block's aligned words, all but at
most three octets at each edge, are read once for their copy and their sum
together, stored as whole words where block and slot lie at one offset in
their words, and the edge octets are copied and summed apart; a producer's data is written by the producer into the slot
itself and summed there. The UDP checksum goes into the headers last. No
owner changes: the block stays the core's until the port's answer, and the
slot stays the port's.

## Counting the core's steps

`demands`, `next_end`, `transmit` and `take_frame` take a `Count`, which
hears the running places a readiness walk visits, each end told, each frame
built whole in the port's slot and each lending refusal for want of credit
with the loans then out: the core's part of the service's switchable
diagnostics (the runtime's README, "Diagnostics"). `()` counts nothing and
leaves no code; an enabled count is asked `Count::on` at each point, and
only counts. The core reads no clock for it, and nothing it decides reads a
count.

## Work records

Each work of a socket has one record, its command place: the data source (a
block, or a producer's declared length), the generation (the command's
serial, which never returns), the state (free, sending, declared,
finished) and, for a declared work, its producer's tag (`T`, `()` for a core
without a producer). Nothing else in the core, or in its producer, holds the
work. Each place but the
close's is on exactly one of three chains while it is free, running or
ended and not yet told, and on none once its end was told and its result
waits for its holder. The chain follows the state, and only a state
transition moves it, so readiness, results and free places are found
without walking every place:

| Transition | Chain change |
| --- | --- |
| `send` or `declare` accepts work | free (or a never-used place) → running, at its tail |
| commit of a declared work whose frame was written | running → free: its outcome, `Accepted`, and its tag come back from `transmit` |
| commit, reject, decline, withdraw, eviction, close, `Unreachable`, `Readdressed` | running → ended, at its tail; an unobserved work frees instead |
| `next_end` tells an end | ended → none: the result stays in the place |
| `next_declared_end` tells a declared work's end | ended → free: its outcome and tag come back once; another work's end on the way is told and stays |
| `take_result`, or `drop_observer` of a finished work | ended or none → free |
| `close` | the close's place → ended, after every running end it caused |
| `retire` | every place → `RetiredResults`, a running one as `Retired`, a declared one with its tag (`take_declared`) |

So a place is never on two chains, and an end is told once: only a running
work goes to the ended chain. A chain holds places, not owners. `demands`, the
neighbors a work protects, `queued` and eviction walk the running chain,
oldest acceptance first. That is the order `demands` enumerates a socket's
work in, subject to readiness and policy: it is no send order. A datagram
whose neighbor is not yet known is not ready, so a later one that is ready
goes before it, and publishing takes what is ready. `next_end(&ends)` tells a socket's ends, oldest first, the close's
among them, as a `WorkId`, which is a place and its generation. It is no right
to the result: that waits in its place for whoever holds the `CommandId`
(`CommandId::work` is the same value), and `take_result` checks the
generation. `SocketId::ends` gives the socket's `Ends`, which outlive its
close. A socket opened again in the same core slot has another serial, so an
earlier socket's `Ends` tells none of its ends.

A local producer's work is declared, not sent: `declare` takes a command place
with `send`'s refusals and no block, keeps the producer's tag in it and returns
the work's `WorkId`, and `open_local` gives the producer a send-only socket, whose port's datagrams are dropped before any block
(`NetEvent::SendOnly`). Its frame's headers go into the port's slot first, then
`transmit`'s `produce` fills the data there, given the tag from the place, and
the UDP checksum, summed over that data in the slot, goes last. A frame the port
took with its data written ends its declared work `Accepted` in that call:
`Transmitted::ended` gives the work and its tag, the place is free, and no end
of it is told later; the frame's own life after the port (its ticket, its air
and its return) is the port's. A producer that declines
(`Declined`) ends the work `Outcome::Declined`, which also passes the turn to
the next socket, and `withdraw_declared` ends every work whose tag the producer
no longer keeps so before any port saw it. A declared work is never queued
for eviction: it holds no block. `neighbors_changed` counts whatever may have
freed a neighbor entry for a new send: an ARP answer, an entry asked again past
its life, a resolution given up or refused, and every end of work that held a
neighbor, so a waiter for `NeighborsFull` keeps the value it saw.

`tick` comes first at every time `poll_deadline` names and at any later `now`
before work is asked for: an address past its life is no work until then.

## Addressing

An interface's address is its owner's (`Interface::new`) or leased by the
core's DHCP client (`Interface::dhcp`, RFC 2131), whose seed the image draws
from its entropy. The core alone holds the address, its version and the
client; `configuration` is a snapshot of them, and `configuration_changes`
moves whenever it may have changed. Any change of address, prefix or gateway
moves `address_version`: every datagram and echo reply accepted under the
old one ends `Outcome::Readdressed` with its block home, and neighbors and
owed ARP replies are forgotten, as they were the old network's. Sockets stay
open: they hold a port on the interface, not an address. A send without an
address is refused `SendRefusal::NoAddress` until the version moves.

The client's messages are the core's own control work: no socket, block or
command place. A message is owed while the link is up (`link`, which the
owner reads from its port each turn), and built in the port's slot when it is
chosen. It is sent when the port takes it: its retransmission time starts
then, it counts toward its phase's backoff and tries, and a lease counts
from its transaction's first REQUEST the port took. A message the port
refuses for good, or one whose server's neighbor never answers, was not
sent: it counts neither as a try nor as a lease's start, and its next
attempt waits for the phase's present time. A first DISCOVER is repeated after 250 ms, doubling to
10 s in one transaction, as a lost first one cost 10 s on the stand
(Xarxa fbebde5); each wait is jittered down by up to a quarter. A REQUEST to
an OFFER is repeated after 5 s, doubling every two, five in all, then
discovery starts over. At T1 the client renews by unicast to the server
identifier, at T2 it rebinds by broadcast, each after half the time left and
no less than 60 s, and at the lease's end the address goes. T1 < T2 < the
lease always holds: the server's when so ordered, else T2 three quarters of
the way from a lone T1, T1 half the lease or half a lone T2 when T2 comes
first, else half and seven eighths. An ACK whose lease, counted from its
REQUEST, is already over is dropped (`LeaseFault::Expired`), and no send
or frame uses an address past its lease's end, before any tick too. A NAK starts
discovery over. A link that comes back at a new epoch runs INIT-REBOOT with a
lease: its broadcast REQUEST names the held address with no server
identifier at once, and repeats on the DISCOVER's schedule, as a rejoin's
first message meets the same race with the access point's key as a first
join's, until an ACK, a NAK or the lease's end. The address stays in use
meanwhile (RFC 2131, 3.7), for configuration and data, but the client stays
REBOOTING (`DhcpState::Rebooting`) until an ACK confirms the lease; the
repeats never extend it. A NAK takes the address away and starts discovery
over; at the lease's end the address goes. Without a lease, a new link
starts discovery over at once.

The client has no address conflict detection: an offered address is bound
on its ACK without an ARP probe (RFC 2131, 2.2 and 4.4.1; RFC 5227), and no
DECLINE is sent. While a client runs, data and echo replies protect at most all
neighbor entries but one, so a renewal always finds one: a send that would
protect an entry data does not protect yet costs a place, cached or not. Messages the client does not
take are counted (`NetEvent::Dhcp`) and change nothing.

An echo request for the interface's own address owes a reply from that
address with all its data (RFC 792, RFC 1122, 3.2.2.6): its ICMP message is
copied into a block, never the pool's last free one, at most `ECHOES` owed
and `ECHO_RATE` accepted a second. A reply waits for its neighbor as a
datagram does, and ends with its block home when the port takes it or
refuses it for good, its neighbor never answers, the address changes or the
core retires; each end that freed its neighbor moves `neighbors_changed`. The
infinite lease of RFC 2131, 3.3 is kept as its value, some 136 years, not as
a lease without end. Everything
else (broadcast echoes, other types, malformed, too long, over a bound) is
counted (`NetEvent::Icmp`) and never answered.

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
