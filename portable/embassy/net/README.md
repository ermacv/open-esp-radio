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

`station_network` consumes a network device once to start the IP stack and
retains that stack and the radio-side runner across station associations.

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

W3 and HIL `wifi-station` select `oer_espressif_executor::ThreadContext<0>`:
their network and station sides remain on the existing P1 executor. Host tests
use `oer_memory::HostContext`. RX ordinary atomic wakers and the packet-pool
waiter are still confined to that composition. These types do not admit new
cross-priority wiring. Timers and polling policy belong to the image.

The current notification directions and priority requirements are in the
[execution audit](../../../docs/execution-wakes.md).
