# oer-esp32s31-ieee80211-driver

The ESP32-S31 Wi-Fi driver: device composition, the lower-MAC port backend,
DMA rings, keys and the P3/P2 interrupt path. Capabilities and their state:
[FEATURES.md](FEATURES.md). System context: [architecture](../../../../docs/architecture.md).

## Rules

- **Driver core.** The driver owns all four EDCA queues and executes each
  attempt's retry plan; a protected MPDU is numbered at its first launch; DMA
  is reached only through its owner type, never alongside a quiescence proof.
- **MAC ownership.** The channel switch writes only the MAC control word and
  the REGDMA link select. TBTT events from the power interrupt are lossy, so
  the driver times its own TBTT wakes. Coexistence values reach the driver by
  message.
- **MAC preparation and start.** Cold preparation programs configuration and
  interrupts with the common MAC gates closed; runtime start requires the
  prepared RX ring's DMA proof, then opens the gates with MAC stop held,
  publishes BASE once, enables the walker and resumes RX, each step after its
  predecessor's proof. Promiscuous policy does not release MAC stop.
- **BSS-color collision** is an HE event, independent of TX queue collisions.
  The station keeps it masked until an HE consumer exists; only a queue's
  `collision_pending` qualifies a TX collision detach.
- **Keys and timeouts.** After an unconfirmed stop the driver erases secrets
  through the abandonment protocol in its own context, and the installation
  stays quarantined until reset. Packet-number exhaustion is a typed refusal
  before backend handoff. No retry after a hardware TX timeout.
- **Protocol RX.** The port validates protected Data through the MAC's CCMP
  validator, retains its public header and PN, and removes the verified MIC
  trailer before protocol delivery. Integrity is hardware evidence only after
  that validation; replay admission remains in the protocol. The borrowed view
  keeps its original DMA lease and on-air length. Raw monitor normalization
  does not assert cipher integrity.
  The CCMP validators execute from `.hot.text.open_radio_rx_hot` in SRAM on
  every protected protocol RX frame. This validation path has a 1056-byte
  additional SRAM placement budget; other SRAM sections and allocated stacks
  retain their existing limits.
- **GTK replacement.** The station keeps one physical group-key slot. A key
  table write failure poisons the port even when an internal rollback restores
  the old material; no success terminal or reusable old GTK is returned.
  The receive handle retains zeroizing rollback material until removal or
  reset. Recovery starts a new association.
- **Station TSF.** An update that would cross 2^64 is refused before any TSF
  write; a beacon whose TSF the station cannot follow is dropped whole.
- **HE trigger-based and beamformee** paths stay fail-closed until a hardware
  path is verified.
- **Conformance.** The hardware lower-MAC conformance image runs every case
  the backend's declared capabilities support; A-MPDU/Block Ack cases without
  `AmpduPhase` are reported as unsupported, never as model-only. The final
  GTK rollback case injects rejection of the new write, lets restoration
  program the real hardware, and compares its retained owners and fail-stop
  with the model. It returns diagnostic evidence without a reusable radio
  owner; the image must reset before another case.

## Radio doorbell

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

## Receive path

When the port's RX handoffs are full, P2 disarms only the RX continuation
timer and keeps the pending RX work and buffer owners. It waits for a
returned lease's RX-capacity fact, a TX event or another applicable deadline;
an overdue RX retry must not keep P2 running while P1 needs to return buffers.
TX deadlines and the port contract are unchanged.

One received frame and the owner of its DMA buffer at each step:

1. **P3.** The MAC line handler masks RX delivery, acknowledges and raises
   the RX fact in the image's static `IrqSignal` of the P2 executor
   (`oer_interrupt_table::signal`), which pends P2 and touches no waker;
   the buffer is the DMA engine's.
2. **P2.** The executor's handler hands the fact to the receive runner
   (`rx/receive.rs`) and wakes it in P2's own context; the runner runs
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
