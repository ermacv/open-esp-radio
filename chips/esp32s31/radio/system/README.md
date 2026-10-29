# oer-esp32s31-radio-system

The ESP32-S31 radio system: one actor future, tied to no executor, that owns
the shared RF front-end and serves its clients' cells
(`oer-esp32s31-radio-client`). It serves Wi-Fi and, with the `bluetooth` feature, one Bluetooth client for
all roles and connections. `attach_bluetooth` lends the same actor to a separately retained Bluetooth
attachment and inbox. Shared prepare, tracking, Wi-Fi service and RF close
keep their original backend type; the attachment never wraps that backend. Wi-Fi-only compositions
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
  tracked. The actor drains inbox requests, then waits on the request doorbell
  versus the backend's tracking deadline. A timer win runs the tick. Requests
  can postpone it; there is no hard-window calendar, one-period deferral bound
  or shift counter. The actor serializes its own retunes with tracking through
  the consuming owned PHY transaction. Registration, tracking, RF close and
  retained wake return the domain with their typed result. Channel selection
  and retune hold the same owned custody across waits. Cancellation
  quarantines its owners and records the operation and registration epoch
  in a 16-byte retained fault before reset.
- A failure that touched shared state poisons the domain; every later answer
  hands back what was lent. The client contract's `Failure` table defines the
  classification.

The hardware steps are a backend (`PhyDomain`, `ClientBackend<K, S>`); host
tests drive the actor over a model backend. The backend owns coexistence and
orders it with the client's power as the hardware requires.

The firmware runs the actor on the P2 radio executor. Constructing it does
not calibrate: the first `join` prepares the unregistered domain through
`register_concurrent_phy`, including cold calibration or an admitted cache
path. A later join after RF close wakes the retained registration. Initial
client tracking settles pending work before operational use. The default
tracking period is 1 s, with the first due microsecond at
`previous + period + 1`. SYSTIMER Alarm0 at P3 acknowledges and pends P2;
the executor dispatches timers and polls the actor.

Actor tasks pin their run future in the executor's retained task storage and
await its pinned reference. Backend operations have a shared non-inlined
poll boundary supplied by PHY's `executor::boundary::Boundary`. One
`owned_operation` helper borrows each pinned owned PHY future and polls
its result into retained storage, then consumes the exclusive `Placed` loan to
return the same domain to its backend slot. Host tests cover cancellation
after registration begins and normal custody return.

PHY's cooperative CPU admission is separate from RF admission. Shared
transport and RX-gain drivers check admission between atoms; direct
TXDC/PWDET paths do not. The 50 µs atom / 1 ms poll contracts are not universal
elapsed-time cutoffs. `admit_maintenance` checks pending tracking state and
the tracking graph programs grant-protect requests without waiting for an
exclusive RF grant. See [PHY execution](../../phy/README.md#runtime-sequence-and-timing).

`Esp32s31Radio` (chip only, `src/esp32s31.rs`) is the ESP32-S31 backend over
the radio domain; its module documentation lists each request's steps. Its
`retune` selects the channel for a stopped tracked client through its
membership cell and returns the measured duration and bounded power profile.
The role owns the MAC/DMA/IRQ stop and restart. `modem_sleep` holds the domain
half of modem sleep, which no client request reaches yet. Under `cfg(oer_trace)` it carries the PHY
registration's diagnostics to the client.

`BluetoothAttachment` retains Bluetooth power/clocks, coexistence and PHY/BTBB
membership beside `Esp32s31Radio`. The client-neutral `ActorCore` keeps the
one RF epoch, membership accounting and poison state. Its closed actions
select RF close without depending on a client or controller type. Its controller adapter keeps generatively scoped
Engine ownership in P2; it never crosses a static Send cell. Initial and due
tracking complete before the affine BLE PHY-ready edge. Runtime handover and
return of a drained, matching-installation retirement occur between separate
mutable borrows of the actor run. A failed or stale retirement cannot release
current membership, clocks or arena. Only a completed last-member exit closes RF.

The attachment loan borrows both permanent owners and `run_until` polls its
composition future only between completed entered transactions. Normal
`detach` returns the same inbox only when the attachment reports stopped;
that operation never moves or resets the shared PHY. Dropping the loan keeps
its seat claimed, so abandonment cannot start a fresh attachment epoch.
A detached and reattached controller does not clear common-domain poison.
