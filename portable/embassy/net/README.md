# Embassy network adapter (owned packets)

`oer-portable-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`). A frame the
stack sends goes straight to the device's transmit half (`NetTransmit`), in the
call that sent it; the crate holds no transmit queue of its own.

The pinned Xarxa fork keeps packet-pool Waker registration in a separate
`#[inline(never)]` frame from the availability recheck and wake. This retains
typed Waker call scopes for the proven stack analysis without changing the
register-then-recheck notification protocol.

Displaced radio readiness registrations wake the previous Waker by reference
only when it would wake a different task, then explicitly drop it; a matching Waker is
only dropped. Separate fixed-slot wake-by-reference and drop calls preserve
the DWARF method scopes the stack analyzer uses to prove Waker targets.

## Product IP service

`ip::Resources<X, RX, TX>` holds final receive pool, endpoint, device and
stack storage. An image places it once and lends it exclusively to `start`,
passing a `NetDevice`, the general allocator of every packet the stack makes
(a device's `ethernet::TxSlots`, so that packets are written in its transmit
memory), interface identity, MAC address, random seed and
`Configuration::Dhcpv4` or `Configuration::Static`. A general allocator
another stack waits on is refused. It receives one local
`Control` and one `Service`; no second start, allocator waiter or stack is
available through that borrow. A construction refusal returns a typed reason.
Storage remains lent.

`Service::run` polls its Ethernet bridge before the pinned stack and publishes
`StatusChanged { previous, current }` synchronously after link/configuration
changes. `Control::status` returns the latest link level/epoch, IPv4 lease or
static configuration, and running/stopped/failed lifecycle. IPv4 configuration
contains address, prefix and optional gateway, using product values rather
than stack types. Idle polls publish no duplicate status. A new epoch revokes
the old DHCP lease before polling the stack, even when Down/Up coalesced to
Up with a new epoch. A changed renewal is observed after each stack poll.
Pools, stack, bridge and socket handles remain in place across reconnects.

`Control::stop` wakes the runner independently of data-plane capacity. The
service lowers its stack link, closes every UDP and raw socket of its stack (dropping what each queued), clears IP
addresses/routes and returns its typed stop/error. It retains its owners in
terminal state; subsequent runs return that same terminal without polling the
stack. Control and runner stay on the same executor/hart. The neutral device's
radio lifecycle is controlled by its own facade, not by stopping IP.

`Service::stack` and `interface` expose the pinned stack for socket
applications; there is no additional TCP/UDP abstraction. HIL only serializes
the service's values. Its bridge preserves HIL's drain-and-report-refusal RX
policy. Frame observations go to the image's
collector and do not become product status counters.

Host service tests inject lease/configuration events at the private stack
boundary and drive protocol deadlines with per-test `oer-portable-time-virtual`
clocks. They also construct the real pinned stack, retain a socket and
configuration across reconnects, and exercise its public terminal paths.
The development-only Embassy mock driver supplies link symbols; these tests
never set or advance its global clock or run the pinned DHCP engine. Existing
bridge cases exercise real packet pools and RX delivery while the device is
full.

A copied RX frame goes into the endpoint's packet pool before publication.
A lent frame becomes a packet in its own storage when the image gives the
bridge its `LentSlots`. The packet's drop only releases its slot; the bridge
drops the owner, which returns the storage to its device, at its next poll
(xarxa's `reclaim`, a direct call), after each stack poll and once more when
the service ends, after its stop closed the sockets that queue packets: a
socket's queued datagrams are lent packets too, and a socket the application
still holds stays closed. A lent packet the application took out of a socket
and still holds after the service ended is never returned. The adapter
forbids unsafe code.

## Initialization and TX state

Queue depths are nonzero compile-time parameters. `split` returns
`EndpointSplitError::AlreadySplit` for resources that already lent their owners,
or `RxWaiterUnavailable` when another consumer owns the RX pool waiter. A
refused waiter claim leaves the endpoint available for a later attempt.

Sending asks `NetTransmit::has_room` at every frame, which registers nothing.
A full device arms `poll_room` with the stack's poll task, so its next room
wakes that task, and every stack poll arms it too, which also takes back the
memory the device returned while nothing is sent. A `true` `can_transmit`
followed by `transmit` finds room; a link-down meanwhile or a frame refused for
its length ends the frame, as one lost on air.

The unique network device and radio runner are neither `Clone`, `Copy`,
`Send` nor `Sync`. The runner stores one RX publisher and one link controller
and lends each through `&mut`; no accessor can mint a second capability. RX
uses `oer-portable-memory::WakingSpscQueue` with its affine producer/consumer ends. A
publisher loan is also local and cannot be cloned. Compile-fail examples check
these ownership restrictions.

Link notification uses `oer_portable_sync::PriorityWaker`; no blocking mutex
or critical section is introduced.

HIL `wifi-station` selects `oer_chips_family_executor::ThreadContext<0>`:
its network and station sides remain on the existing P1 executor. Host tests
use `oer_portable_memory::HostContext`. RX ordinary atomic wakers and the packet-pool
waiter are still confined to that composition. These types do not admit new
cross-priority wiring.

## Event-driven bridges

`ethernet::EthernetBridge` has no periodic timer.
Each turn registers the neutral device's link waiter before observing the
link level and epoch, even with empty queues. The HIL bridge services RX
whatever transmission does, retaining its drain-and-report-refusal RX policy.

| Waiting state | Event source |
| --- | --- |
| No device RX | `NetDevice::poll_receive_ready` |
| Link level or association epoch changes | `NetDevice::poll_link`; registration precedes the snapshot |
| Device full, or its memory returned | `NetTransmit::poll_room` with the stack's poll task |
| Stack RX or link | owned device's stack waker |

An idle or blocked turn does not wake itself. The image polls its network
event producers before its application predicates in the same task; their
shared waker delivers link, DHCP and probe updates without a 10-ms sampler.
The IP service compares the lease after each stack-runner poll, including renewal while
the interface remains configured. Only protocol and application deadlines
use time.

The current notification directions and priority requirements are in the
[execution audit](../../../docs/execution-wakes.md).
