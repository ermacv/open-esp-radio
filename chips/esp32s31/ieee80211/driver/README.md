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
- **RX storage.** The large-RX profile uses 96 ring buffers and one reserved
  buffer R, each with 4608 backing bytes. Accepted units contain the descriptor
  interval from the software cursor through its first terminal RX_DONE inside
  frozen LAST. Descriptor lengths and ownership grants govern completion and
  recycle; preparation and recycle preserve buffer bytes. A cold cursor at R
  includes R in the first unit until its terminal, while a retained ring NEXT
  starts directly in the ring. SOURCE[HW18_RX_COMMITTED_PREFIX_20261027] in
  `chips/esp32s31/registers/evidence/hw-rx-radio.toml` supports the exact-prefix
  hardware assumption for three measured accepted chains.
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

## TX CONTROL bit 22

The HAL's legacy, HT and HE formatter sets queue CONTROL bit 22 on every
prepared publication. This is a known difference from the vendor's
reserved-TXOP-slot adjustment; the bit's physical meaning and effect on air
remain unqualified.

SOURCE: `VENDOR_WIFI_TXOP_DESCRIPTOR_CLASS` in
[`vendor-opaque-identities.toml`](../../registers/evidence/vendor-opaque-identities.toml),
from pinned `esp32-wifi-lib@f856b5db55cb18e2d1826044430f3f4089e47cf9`.
`hal_mac_tx.o::mac_tx_set_plcp0` sets bit 22. The subsequent
`mac_tx_set_txop_q` first tests queue-context byte `+0x1d`: initialized or
released index 3 preserves the bit; reserved slot 0/1/2 sets it only when
the head software-descriptor flags `[7:6]` equal 2 and clears it for 0/1/3.
`lmacSetTxFrame` can reserve a slot for unfragmented traffic, so a frame
class or cipher alone does not determine the final bit.

All inspected vendor unfragmented construction paths retain flags `[7:6]`
zero: plaintext data, probe/authentication/association management, pre-key
EAPOL, encrypted and unencrypted ESP-NOW, CCMP/GCMP/TKIP/WEP data and BIP
group management. Successful software-fragment chains use 2 before the
last fragment and 3 on the last fragment. Cipher/key selectors occupy the
separate descriptor word `+0x10`; CCMP does not imply flags class 2.
These are bounded source observations; they establish no air equivalence.

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

MAC source120 `FATAL_ASSERTION` (bit21) names the vendor's fatal assertion
path, without identifying a physical producer. The handler checks unmasked RAW
before acknowledging STATUS; every epoch clear checks RAW with both banks
masked before clearing either bank, acknowledging only that same nonfatal MAC
RAW sample so a later fatal arrival remains pending. This includes activation,
direct close, `MaskedBanks::clear_all` and a stray entry without an installed
line. A fatal cause returns the MAC crate's `irq::Fault::FatalAssertion { raw }`
and keeps both banks quarantined. Closing or activating a quarantined epoch returns the
same fault without clearing it. While Masked (or Closed), a new fatal cause
remains in RAW; detection waits for a later handler entry, which that masked
cause alone does not guarantee.

Staged Wi-Fi images select the runtime's `wifi-fault` feature and pass the RAW
sample to `wifi_fault::reset`, retaining their interrupt and preempted DMA
owners. That shared module owns one twelve-byte retained record and calls
`fatal::reset_recording`; after `Root::into_runtime` installs the early console,
its `report_previous` consumes and prints the reason and complete RAW word.
The ordinary record stores, magic and checksum are not a power-fail commit
protocol. Policy dispatch uses the PAC's named cause and never decodes the
diagnostic word. SOURCE: `BLOB_LIBPP_WIFI_IRQ_FACTS_20261026` (the vendor's
fatal diagnostic message14), `BLOB_LIBPP_ISR_REGISTER_DUMP` (RAW bank).

Power source122 masking preserves pending in RAW, while masked STATUS hides
it for the measured TSF timer0/1 lanes. Epoch closure uses the existing
mask-and-clear-all transaction, rather than sampling STATUS after masking.
The host model tests masked STATUS, redelivery and clearing with a positive
sample retained before masking. Its TBTT model remains a policy assumption;
the timer stand observations do not qualify received-beacon causes.

P2 asks for a bank change by setting a request bit (`fetch_or`, Release)
and ringing the doorbell; the handler preempts P2, so a request that no
longer reads pending has been applied. The handler lowers the doorbell
first, then takes every request at once (`swap(0)`, AcqRel), so a ring
after the lowering raises it again and requests before the take coalesce;
the safest wins. `Close` masks and clears both banks (the epoch closes).
`MaskAll` masks both and keeps their latched events, and posts the masked
fact once. The activation (`WifiIrqRequests::activate`, which takes the
router's `Bound` proof of the part) masks, clears the nonfatal MAC RAW sample
and WDEVPWR, then publishes the station mask and the station TBTT. `UnmaskRx`
restores the RX delivery group of an active epoch only; otherwise it is ignored and
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
   (`oer_espressif_interrupt_table::signal`), which pends P2 and touches no waker;
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
