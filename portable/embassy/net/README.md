# Embassy network adapter (owned packets)

`oer-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`).

The pinned Xarxa fork keeps packet-pool Waker registration in a separate
`#[inline(never)]` frame from the availability recheck and wake. This retains
typed Waker call scopes for the proven stack analysis without changing the
register-then-recheck notification protocol.

Displaced TX registrations likewise wake or drop their previous Waker in the
private, never-inlined `wake_replaced` helper. This keeps their stack-analysis
boundary separate from the enclosing network poll. In the W3 composition,
the helper's two indirect tail exits still lack a complete Waker proof;
the separate boundary alone does not prove its stack bound.

RX frames are copied into the endpoint's packet pool before publication.
The adapter accepts packet-pool owners and forbids unsafe code; it does not
adopt detached DMA storage.

## Initialization and TX state

Queue depths are nonzero compile-time parameters. `split` returns
`EndpointSplitError::AlreadySplit` for resources that already lent their owners,
or `RxWaiterUnavailable` when another consumer owns the RX pool waiter. A
refused waiter claim leaves the endpoint available for a later attempt.

TX metadata and admission state use `Cell<State>` inside the existing raw
mutex. Each synchronous `with_state` closure takes the state into its local
`state`, updates it, and restores it before leaving the mutex. Waker cloning,
waking and destruction happen outside that closure. This lets a wake callback
inspect or drain the restored queue, or immediately spend a returned credit.
Each dequeued frame retains its unique admission credit until destruction;
packet destruction precedes the capacity wake.

For `riscv32imafc-unknown-none-elf`, both current image compositions (W3 and
HIL `wifi-station`) use RX depth 8 and TX depth 4. `tx_queue::State<4>` is
436 bytes; `tx_budget::State` is 12 bytes. Their synchronous local values live
on the P1 `station_task` stack while its network/bridge or stack poll calls TX
methods. They are not retained across an await. Permanent state remains in
`ENDPOINT`; taking it requires an O(TX depth) temporary. These are type sizes,
not full frame sizes or stack bounds: compiler moves/inlining may change the
actual frames. CI reports memory use and enforces the images' stack proofs.

## Known defects

- **Contract: the RX capability is copyable.** `owned.rs::OwnedRxPublisher`
  derives `Clone` and `Copy` ("Copyable RX-only capability"), so any number
  of copies can publish into the endpoint's RX queue, against one owner per
  resource.
- **Contract: queues and wakers lock through a blocking mutex.** The RX
  channel (`embassy_sync` `Sender`/`Receiver`), TX metadata and `OwnedLinkState`'s
  `GenericAtomicWaker<M>` lock a blocking `Mutex<M: RawMutex>`; the
  composition picks `M`. W3 uses `NoopRawMutex` and polls the network and
  station users on P1; tests also use `NoopRawMutex`. With
  `CriticalSectionRawMutex` every lock masks all
  interrupts, including the P3 radio handlers, which is directly against the
  execution rule (no critical section on P2/P3, no lock shared between a
  handler and a task). `NoopRawMutex` is sound only when every user runs on
  one executor, which the adapter's types do not enforce.

The current notification directions and priority requirements are in the
[execution audit](../../../docs/execution-wakes.md).
