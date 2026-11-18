# oer-chips-esp32s31-radio-client

The client contract of the ESP32-S31 radio system (docs/architecture.md,
"Radio system"): what a protocol driver needs to be a member of the shared
PHY domain, without the actor itself.

- `claim()` returns the actor's end and the clients' cells once per image.
  Each client has one `MembershipCell`; `join`, `track`, `retune`, `leave`
  and `reset_mac` post synchronously and return a `Pending` future of the
  answer and the cell.
- The cell is lock-free: it holds the `oer-portable-memory` `RequestSlot`, a
  four-state CAS protocol with two atomic wakers (`docs/unsafe.md`); the
  crate forbids unsafe code.
- `Member<K, Untracked | Tracked>` is the PHY membership typestate; only
  `track` produces `Tracked`, which alone permits RF use. `RfEpoch` changes at
  every registration and RF wake.
- A failed request hands back what it lent; `Failure` (`Refused`,
  `ClientFailed`, `PhyRefused`, `PoisonedAfterRefusal`, `Poisoned`) says what the failure touched, as the table on
  `Failure` defines.
- `LpClock<RcSlow>` is the board's low-power clock, claimed once.
- `wifi` holds the Wi-Fi payloads. `track` is the initial tracking, the RX
  enable and the initial channel; the driver lends its clocked client and
  gets it back tuned. A tracked member's `reset_mac` lends the proof of its
  clocks (`WifiClocksOn`) and the tuned channel and gets back the proof, the
  MAC block's reset proof (`MacBlockReset`, neither `Copy` nor `Clone`) and
  the coexistence priorities of its cold preparation; it has no refusal and
  no failure, and is answered in a poisoned domain too. The cold preparation
  is the member's own; `MacColdSources` holds what it reads. Bluetooth's
  `ResetMac` is uninhabited.
- With `bluetooth`, `bluetooth::claim()` supplies the second cell for the same
  actor. Its requests carry configuration and installation numbers; the BLE
  backend retains membership and controller memory. `Permission` derives the
  ceiling from the exact LE band, rejects transmission under passive-only
  authority, and rounds permitted TxPower down to a provider level.
  `bluetooth::RadioController` and its failure types define the narrow
  controller lifecycle contract implemented by the driver and used by the
  shared actor, without a radio-system dependency on the driver.
- `ACTOR_STEP_BOUND` is the provisional 50 µs bound of one actor poll; the
  request bounds follow from measurements.
