# Embassy network adapter (owned packets)

`oer-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`).

The pinned Xarxa fork keeps packet-pool Waker registration in a separate
`#[inline(never)]` frame from the availability recheck and wake. This retains
typed Waker call scopes for the proven stack analysis without changing the
register-then-recheck notification protocol.

RX frames are copied into the endpoint's packet pool before publication.
The adapter accepts packet-pool owners and forbids unsafe code; it does not
adopt detached DMA storage.

## Known defects

- **Contract: the RX capability is copyable.** `owned.rs::OwnedRxPublisher`
  derives `Clone` and `Copy` ("Copyable RX-only capability"), so any number
  of copies can publish into the endpoint's RX queue, against one owner per
  resource.
- **Contract: queues and wakers lock through a blocking mutex.** The RX and TX
  channels (`embassy_sync` `Sender`/`Receiver`) and `OwnedLinkState`'s
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
