# oer-esp32s31-radio-client

The client contract of the ESP32-S31 radio system (docs/architecture.md,
"Radio system"): what a protocol driver needs to be a member of the shared
PHY domain, without the actor itself.

- `claim()` returns the actor's end and the clients' cells once per image.
  Each client has one `MembershipCell`; `join`, `track` and `leave` post
  synchronously and return a `Pending` future of the answer and the cell.
- The cell is lock-free: it holds the `oer-memory` `RequestSlot`, a
  four-state CAS protocol with two atomic wakers (`docs/unsafe.md`); the
  crate forbids unsafe code.
- `Member<K, Untracked | Tracked>` is the PHY membership typestate; only
  `track` produces `Tracked`, which alone permits RF use. `RfEpoch` changes at
  every registration and RF wake.
- A failed request hands back what it lent; `Failure` (`Refused`,
  `ClientFailed`, `Poisoned`) says what the failure touched, as the table on
  `Failure` defines.
- `LpClock<RcSlow>` is the board's low-power clock, claimed once.
- `wifi` holds the Wi-Fi payloads. The actor brings the Wi-Fi MAC up in
  `track`, after the initial tracking, the RX enable and the initial channel;
  the driver lends its clocked client for that request and gets it back with
  the MAC up.
- `ACTOR_STEP_BOUND` is the provisional 50 µs bound of one actor poll; the
  request bounds follow from measurements.
