# oer-esp32s31-radio-system

The ESP32-S31 radio system: one actor future, tied to no executor, that owns
the shared RF front-end and serves its clients' cells
(`oer-esp32s31-radio-client`). R0 serves Wi-Fi.

- `join`: prepare the PHY domain (register it, or wake closed RF), mint a new
  RF epoch when RF opened, enable coexistence, enter the client. A refused
  first member closes RF again.
- `track`: the initial tracking; the member becomes `Tracked`.
- `leave`: exit the client; the last member's exit mints `LastClientLeft`,
  which only `close_rf` consumes.
- Between requests: the periodic tracking tick, when due, while a member is
  tracked.
- A hardware failure poisons the domain; every later answer hands back what
  was lent.

The hardware steps are a backend (`PhyDomain`, `ClientBackend<K, S>`); host
tests drive the actor over a model backend.
