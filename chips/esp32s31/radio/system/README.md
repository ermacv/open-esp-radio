# oer-esp32s31-radio-system

The ESP32-S31 radio system: one actor future, tied to no executor, that owns
the shared RF front-end and serves its clients' cells
(`oer-esp32s31-radio-client`). It serves Wi-Fi and, with the `bluetooth` feature, one Bluetooth client for
all roles and connections. `with_bluetooth` adds its inbox to the same actor
future, shared backend, tracking and poisoning state. Wi-Fi-only compositions
contain no Bluetooth state.

- `join`: prepare the PHY domain (register it, or wake closed RF), mint a new
  RF epoch when RF opened, enter the client. A first member that fails
  without poisoning closes RF again.
- `track`: the initial tracking and what the client contract adds to it; the
  member becomes `Tracked`.
- `leave`: exit the client; the last member's exit mints `LastClientLeft`,
  which only `close_rf` consumes. A recoverable close leaves RF open in the
  same epoch.
- Between requests: the periodic tracking tick, when due, while a member is
  tracked.
- A failure that touched shared state poisons the domain; every later answer
  hands back what was lent. The client contract's `Failure` table defines the
  classification.

The hardware steps are a backend (`PhyDomain`, `ClientBackend<K, S>`); host
tests drive the actor over a model backend. The backend owns coexistence and
orders it with the client's power as the hardware requires.

`Esp32s31Radio` (chip only, `src/esp32s31.rs`) is the ESP32-S31 backend over
the radio domain; its module documentation lists each request's steps. Its
`retune` selects the channel for a stopped tracked client through its
membership cell and returns the measured duration and bounded power profile.
The role owns the MAC/DMA/IRQ stop and restart. `modem_sleep` holds the domain
half of modem sleep, which no client request reaches yet. Under `cfg(oer_trace)` it carries the PHY
registration's diagnostics to the client.

`WithBluetooth` adds Bluetooth shared power/clocks, coexistence and PHY/BTBB
membership to `Esp32s31Radio`. Its controller adapter keeps generatively scoped
Engine ownership in P2; it never crosses a static Send cell. Initial and due
tracking complete before the affine BLE PHY-ready edge. Runtime handover and
return of a drained, matching-installation retirement occur between separate
mutable borrows of the actor run. A failed or stale retirement cannot release
current membership, clocks or arena. Only a completed last-member exit closes RF.
