# Embassy network adapter (owned packets)

`oer-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`). Its
`datapath` module holds the software/physical TX ownership contracts, the
destination/flow queue arena `TxQueues` and, under the `model` feature, the
host model `MemoryTxQueues`.

The pinned Xarxa fork keeps packet-pool Waker registration in a separate
`#[inline(never)]` frame from the availability recheck and wake. This retains
typed Waker call scopes for the proven stack analysis without changing the
register-then-recheck notification protocol.

Displaced radio readiness registrations wake the previous Waker by reference
only when it would wake a different task, then explicitly drop it; a matching Waker is
only dropped. Separate fixed-slot wake-by-reference and drop calls preserve
the DWARF method scopes the stack analyzer uses to prove Waker targets.

## Product IP service

`ip::Resources<X, RX, TX, GENERAL>` holds final pool, endpoint, device and
stack storage. An image places it once and lends it exclusively to `start`,
passing a `NetDevice`, interface identity, MAC address, random seed and
`Configuration::Dhcpv4` or `Configuration::Static`. It receives one local
`Control` and one `Service`; no second start, allocator waiter or stack is
available through that borrow. A construction refusal returns the original
neutral device and a typed reason. Storage remains lent.

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
service lowers its stack link, reclaims pending/queued software TX, clears IP
addresses/routes and returns its typed stop/error. It retains its owners in
terminal state; subsequent runs return that same terminal without polling the
stack. Control and runner stay on the same executor/hart. The neutral device's
radio lifecycle is controlled by its own facade, not by stopping IP.

`Service::stack` and `interface` expose the pinned stack for socket
applications; there is no additional TCP/UDP abstraction. HIL only serializes
the service's values. Its bridge preserves HIL's drain-and-report-refusal RX
policy and serves one TX per turn. Frame observations go to the image's
collector and do not become product status counters.

Host service tests inject lease/configuration events at the private stack
boundary and drive protocol deadlines with per-test `oer-time-virtual`
clocks. They also construct the real pinned stack, retain a socket and
configuration across reconnects, and exercise its public terminal paths.
The development-only Embassy mock driver supplies link symbols; these tests
never set or advance its global clock or run the pinned DHCP engine. Existing
bridge cases exercise real packet pools, TX credit and RX delivery during
TX backpressure.

RX frames are copied into the endpoint's packet pool before publication.
The adapter accepts packet-pool owners and forbids unsafe code; it does not
adopt detached DMA storage.

## Initialization and TX state

Queue depths are nonzero compile-time parameters. `split` returns
`EndpointSplitError::AlreadySplit` for resources that already lent their owners,
or `RxWaiterUnavailable` when another consumer owns the RX pool waiter. A
refused waiter claim leaves the endpoint available for a later attempt.

TX destination/flow metadata and the coalesced publication hint live in
`ContextCell<State, Entered<X>>`. The private proof checks the selected
`ExecutionContext` on every access. State is borrowed in place, without the
O(TX depth) stack temporary. `AccessRefused` reports a wrong context or nested
borrow without consuming queued owners; a refused TX publication returns the
original packet. Queue queries and waits propagate that typed refusal.

The unique network device and radio runner are neither `Clone`, `Copy`,
`Send` nor `Sync`. The runner stores one RX publisher and one link controller
and lends each through `&mut`; no accessor can mint a second capability. RX
uses `oer-memory::WakingSpscQueue` with its affine producer/consumer ends. A
publisher loan is also local and cannot be cloned. Publication waits retain
an exclusive runner borrow, so a second outstanding wait cannot replace them.
Compile-fail examples check these ownership restrictions.

Admission is an atomic count independent of the metadata borrow. Each dequeued
frame retains its unique credit through radio retention; destruction returns
the packet pool slot before releasing the credit and waking the producer.
Thus dropping a retained packet does not require entering the metadata context.
Link and capacity notification use `oer_sync::PriorityWaker`; no blocking mutex
or critical section is introduced. Queue Waker clone, wake and drop callbacks
run only outside the ContextCell borrow, including displaced registrations.

HIL `wifi-station` selects `oer_espressif_executor::ThreadContext<0>`:
its network and station sides remain on the existing P1 executor. Host tests
use `oer_memory::HostContext`. RX ordinary atomic wakers and the packet-pool
waiter are still confined to that composition. These types do not admit new
cross-priority wiring.

## Event-driven bridges

`ethernet::EthernetBridge` has no periodic timer.
Each turn registers the neutral device's link waiter before observing the
link level and epoch, even with empty queues or a retained TX. An epoch
change reclaims stale retained and queued owners before waiting for new TX
capacity. The HIL bridge continues servicing RX while TX is blocked, retaining
its drain-and-report-refusal RX policy.

| Waiting state | Event source |
| --- | --- |
| No device RX | `NetDevice::poll_receive_ready` |
| No stack TX | coalesced TX publication hint; consuming a stale hint schedules a turn to rearm |
| Retained TX blocked or refused | `NetDevice::poll_transmit_ready`, rearmed after refusal |
| Stack TX admission full | packet destruction, then credit return and producer wake |
| Link level or association epoch changes | `NetDevice::poll_link`; registration precedes the snapshot |
| Stack RX, link or returned TX credit | owned device's stack waker |

After work, a continuation yields to the other P1 futures and rearms waits.
An idle or blocked turn does not wake itself. The image polls its network
event producers before its application predicates in the same task; their
shared waker delivers link, DHCP and probe updates without a 10-ms sampler.
The IP service compares the lease after each stack-runner poll, including renewal while
the interface remains configured. Only protocol and application deadlines
use time.

The current notification directions and priority requirements are in the
[execution audit](../../../docs/execution-wakes.md).
