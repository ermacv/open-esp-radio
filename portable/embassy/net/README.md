# Embassy network adapter (owned packets)

`oer-embassy-net` is the owned-packet boundary between the Wi-Fi
datapath and the Embassy/Xarxa network stack (`docs/network.md`).

## Optional external RX

`external-rx` enables `ExternalRxOrigin`, admission and accounting for detached
DMA payloads. It forwards to `xarxa-driver/external-origin` and is disabled by
default. Firmware images use ordinary packet pools; their packet release path
then contains no external owner callback. Enable the feature to run the host
external-adoption tests:

```sh
cargo test -p oer-embassy-net --features external-rx
```

The host CI runs both the default and external-RX feature sets.

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
