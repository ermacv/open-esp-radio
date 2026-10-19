# Embassy network adapter (owned packets)

`oer-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`).

## Known defects

- **Contract: the RX capability is copyable.** `owned.rs::OwnedRxPublisher`
  derives `Clone` and `Copy` ("Copyable RX-only capability"), so any number
  of copies can publish into the endpoint's RX queue, against one owner per
  resource.
- **Contract: queues and wakers lock through a blocking mutex.** The RX and TX
  channels (`embassy_sync` `Sender`/`Receiver`) and `OwnedLinkState`'s
  `GenericAtomicWaker<M>` lock a blocking `Mutex<M: RawMutex>`; the
  composition picks `M` (no composition in this repository yet; tests use
  `NoopRawMutex`). With `CriticalSectionRawMutex` every lock masks all
  interrupts, including the P3 radio handlers, which is directly against the
  execution rule (no critical section on P2/P3, no lock shared between a
  handler and a task). `NoopRawMutex` is sound only when every user runs on
  one executor, which the adapter's types do not enforce.
