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
installed and silenced right after the mask is confirmed.

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

## Placement

Code goes where the knowledge of the hardware is: `crates/esp32s31/` for this
chip's registers, `crates/espressif/` for Espressif IP shared by their chips,
the protocol directories for portable code, `crates/base/`, `radio/` and
`exec/` for the rest, `crates/adapters/` for bindings. See `CLAUDE.md`.
