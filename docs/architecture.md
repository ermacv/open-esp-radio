# Architecture

This page describes what exists and the rules new code follows. It grows as
the Wi-Fi slice reaches hardware; [decisions](decisions.md) records why.

## Layers

```text
application          credentials, network stack (embassy-net), sockets
  ieee80211 runtime  one async loop per role set, over the lower-MAC port   (W3)
  protocols          sans-IO state machines: frames, MAC, RSN, station, AP,
                     BLE link layer and HCI, IEEE 802.15.4 MAC
  radio port         lifecycle and capacity vocabulary shared by the ports
  chip drivers       ESP32-S31 Wi-Fi DMA, MAC and driver core
  radio system       one owner of the shared RF front-end              (R0..)
  PHY                calibration, tracking, channel, TX power (Rust port of libphy)
  HAL                typed register ownership: root, partitions, phased sequences
  PAC                generated from registers/ (raw) plus single transactions
platform (firmware/) staged boot, interrupt table, panic record, linker layout
```

Protocols are **sans-IO**: frames, completed operations and the current time
enter as values; actions and the next deadline leave as values. They never
await, so they run under any executor and in host tests with virtual time
(`oer-time-virtual`). Drivers own hardware and wait; adapters bind executors
and `embassy-time`.

## Execution contexts

| Context | Runs | Rule |
| --- | --- | --- |
| P3, core 0 | radio interrupt handlers; one shared radio doorbell on `FROM_CPU` line 3; the IEEE 802.15.4 engine (192 µs turnaround) | Wi-Fi and BLE handlers only latch status and post events |
| P2 | one radio executor on `FROM_CPU` line 2: Wi-Fi driver, BLE lower link layer, the radio-system actor | every step bounded; long waits are timer waits |
| P1 / thread | role bodies, network stack, application | — |

An interrupt handler never shares a lock with a task. Drivers never block or
spin. Lines `FROM_CPU` 0 and 1 are the per-core executors.

### Radio doorbell

While a Wi-Fi interrupt epoch is live, only P3 writes the MAC and WDEVPWR
interrupt banks. The P3 handlers run one `WifiIrqLine` (Wi-Fi driver
`irq.rs`) through a `HandlerCell`, an ownership state machine rather than a
lock: the line holds the active epoch, the fact publishers and a request
word. P2 asks for a bank change by setting a request bit (`fetch_or`,
Release) and ringing the doorbell. The doorbell handler resets its software
interrupt first, then takes every request at once (`swap(0)`, AcqRel), so a
ring after the reset raises it again and requests before the take coalesce.
`MASK_ALL` wins over `UNMASK_RX`: the line masks both banks once and posts
the masked fact; an unmask after it is ignored and counted. P2 requests
`UNMASK_RX` only after the RX frontier drained with no released buffer left
to rearm. Installing a line clears a stale request. The doorbell route is
enabled at boot; the MAC and WDEVPWR routes are enabled after the line is
installed and silenced right after the mask is confirmed. A lease the upper
layer returns posts the RX-capacity fact (`RxReturnNotifier`), so the runner
rearms the returned descriptors even when an exhausted walker raises no RX
interrupt; the notice touches no ring state.

### W1 receive path

One received frame and the owner of its DMA buffer at each step:

1. **P3.** The MAC line handler masks RX delivery, acknowledges and posts
   the RX fact; the buffer is the DMA engine's.
2. **P2.** The receive runner (`rx/receive.rs`) wakes on the fact and runs
   the product service (`rx/transaction`), which takes the completed unit
   and stages it: the original buffer becomes a `NetworkRxFrame` lease of
   the staging pool, with no copy. Chained, oversized, empty and too-long
   units are dropped and counted there and never leave P2.
3. **P2.** `QueueSink` (`rx/upper.rs`), the runner's publisher, marks the
   lease's first handoff (the monotonic time and the prefix's raw MAC
   stamp), counts it and sends it into an affine SPSC queue as deep as the
   pool; a send after a pool credit therefore never refuses. The queue owns
   the lease. The runner unmasks RX through the doorbell once drained.
4. **P1.** `FrameReceiver::next` takes the lease and holds it as a
   `HeldRxFrame`; `normalize` decodes the prefix under `W1_INGRESS` (no
   strict RXEND or dump-length refusal; what they would refuse is counted)
   and counts refusals by reason. The application owns the lease.
5. **P1.** The MPDU and `portable::rx_meta` on the configured channel go to
   `sta::receive::classify` and `observe_scan`; in W1 the acceptance
   instrument `runtime::survey::Survey` calls them and tallies each BSS.
6. **P1.** Dropping the held frame releases the lease and posts the
   RX-capacity fact; P2 rearms the buffer.

P2 and P1 share only the queue's cursors and an `AtomicWaker`. Teardown waits,
bounded, until no lease is staged before it stops the walker, and queued
frames are staged: P1 keeps receiving and dropping frames after it requests
the stop, until the exit is published; otherwise the epoch ends quarantined
with `DrainTimeout`.

## Ownership

- One owner per resource, neither `Clone` nor `Copy`: `&mut` within a call, a
  token or lease beyond it, an actor across tasks. A handle shared over a mutex
  is not ownership.
- Who controls a state decides its form: the caller's own state is a typestate
  or token, the world's (air, hardware, peers, time) a typed refusal or outcome.
  A `Busy` the caller's own state explains is a design defect.
- A guarantee is written down only with what enforces it (a type with a
  compile-fail test, or a stated run-time check).
- DMA-reachable storage is permanent and is released only after a proof that
  DMA stopped; otherwise it is quarantined until reset
  ([rules](../knowledge/esp32s31/wifi/dma-execution.md)).

## Hardware ownership

Two roots, split by phase. esp-hal (the owner's fork) owns the SoC and
everything before the radio handoff: boot, clocks, interrupts, PSRAM, flash,
console, time. Our HAL owns the radio blocks after the handoff. Each word both
sides can write has one runtime writer, listed and reviewed in
`registers/esp32s31/shared-words.toml` and checked by
`cargo registers check shared-words`.

Structures the hardware reads and writes in RAM (DMA descriptors, the RX
control prefix, TX descriptor words, BLE exchange memory) are part of the
hardware too: [hardware memory](hardware-memory.md) proposes describing them
in the register model, with the protocol in the HAL.

## Radio system

One actor on the P2 executor will own the shared RF front-end: PHY
registration, calibration and tracking, channel retune, modem clocks,
coexistence timers and sleep. Clients reach it through per-client cells with
lock-free wakers; PHY membership is a typestate (`Untracked`, `Tracked`,
`Suspended`). It owns the HAL's `RadioDomain` by value. It is built in steps
with the Wi-Fi slice (R0 at W0); coexistence claims arrive with the second
radio.

## Radio port

`oer-radio-port` and the Wi-Fi lower-MAC port (`oer-ieee80211-lower-mac`) are
carried unfrozen. Steps W0–W2 drive the chip driver directly; the port backend
is written at W3 and the contract is frozen after W4.
[Radio ports](radio-ports.md) states the principles they follow,
[time](time.md) the clock domains and their relations.

## Placement

Code goes where the knowledge of the hardware is: `crates/esp32s31/` for this
chip's registers, `crates/espressif/` for Espressif IP shared by their chips,
the protocol directories for portable code, `crates/base/`, `radio/` and
`exec/` for the rest, `crates/adapters/` for bindings. See `CLAUDE.md`.
