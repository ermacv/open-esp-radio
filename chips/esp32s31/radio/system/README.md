# oer-esp32s31-radio-system

The ESP32-S31 radio system: one actor future, tied to no executor, that owns
the shared RF front-end and serves its clients' cells
(`oer-esp32s31-radio-client`). R0 serves Wi-Fi.

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
`retune` and `modem_sleep` modules hold the domain halves of the channel
retune (R1) and of modem sleep (with doze, R3), compiled but not yet part of
the client contract. The `registration-diagnostics` feature carries the PHY
registration's diagnostics to the client.
