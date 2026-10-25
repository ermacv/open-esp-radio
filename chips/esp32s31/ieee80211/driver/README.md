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
- **MAC preparation and start.** Cold preparation programs configuration
  with the common MAC gates closed and never writes an interrupt bank;
  runtime start requires the
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

## Radio interrupt part

One entry of the image's interrupt table binds `WifiIrqPart` once, at start:
its sources are the MAC bank (`MODEM_WIFI_MAC`), the WDEVPWR bank
(`MODEM_WIFI_PWR`) and the radio doorbell (`FROM_CPU_INTR3`), all routed to
the entry's one P3 line. Its handler holds the part, with both banks in
their interrupt epoch (`irq::epoch::InterruptEpoch` of the MAC crate), for
the image's life; no task writes a bank and the CPU route never changes.
The handler calls `service_wifi_line` and serves one cause per entry, read
from the peripherals: it lowers the doorbell and applies the requests it
takes; without one it serves the MAC bank, or else the WDEVPWR bank. The
line is level-triggered, so a cause left for later enters it again at once.
The arena's fact publishers, request word and counters reach the handler as
one `WifiIrqLine` the task installs once in a `HandlerCell`, an ownership
state machine rather than a lock.

P2 asks for a bank change by setting a request bit (`fetch_or`, Release)
and ringing the doorbell; the handler preempts P2, so a request that no
longer reads pending has been applied. The handler lowers the doorbell
first, then takes every request at once (`swap(0)`, AcqRel), so a ring
after the lowering raises it again and requests before the take coalesce;
the safest wins. `Close` masks and clears both banks (the epoch closes).
`MaskAll` masks both and keeps their latched events, and posts the masked
fact once. The activation (`WifiIrqRequests::activate`, which takes the
router's `Bound` proof of the part) masks, clears every stale event, then
publishes the station mask and the station TBTT. `UnmaskRx` restores the RX
delivery group of an active epoch only; otherwise it is ignored and
counted. Clearing takes the masking proof (`MaskedBanks`). A closed epoch
that still sees an enabled event (a reset restored an enable) closes it
again. After each request the handler reads the MAC enable image and
publishes it (`WifiIrqRequests::mac_interrupt_enable`).

A start activates the epoch; a pause or retune closes it after the mask,
the drain and the walker stop; a freeze masks it and its resume activates
it again. P2 requests `UnmaskRx` only after the RX frontier drained with no
released buffer left to rearm. Installing a line clears a stale request. A
lease the upper layer returns posts the RX-capacity fact
(`RxReturnNotifier`), so the runner rearms the returned descriptors even
when an exhausted walker raises no RX interrupt; the notice touches no ring
state.

## Receive path

When the port's RX handoffs are full, P2 disarms only the RX continuation
timer and keeps the pending RX work and buffer owners. It waits for a
returned lease's RX-capacity fact, a TX event or another applicable deadline;
an overdue RX retry must not keep P2 running while P1 needs to return buffers.
TX deadlines and the port contract are unchanged.

One received frame and the owner of its DMA buffer at each step:

1. **P3.** The radio handler masks RX delivery, acknowledges and raises
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
