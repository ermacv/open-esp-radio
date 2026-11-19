# oer-chips-esp32s31-radio-ieee80211-backend

The ESP32-S31 Wi-Fi hardware code: the MAC's sequences, its DMA rings and
queues, keys, the receive session and the path from the radio-level interrupt to the
task that owns the line. System context: [architecture](../../../../../docs/architecture.md).

The backend has no port: nothing here implements the port contract, and no
Wi-Fi image links over it. What the MAC
offers the contract is in `facts`: the capabilities
(`esp32s31_lower_mac_capabilities`, `ESP32S31_BEACON_TIMING_CAPABILITIES`),
the queue, interface and key-slot counts, a received MPDU's metadata
(`Esp32s31RxMeta`), the HE association's mapping onto the MAC's fields
(`mac_he_association`), the coexistence priorities a frame carries
(`RadioCoexPriorities`), and the register seams of the station TBTT, the
transmit gate, the beacon receive priority and the HE association, which
`cooperative_hardware` implements on the chip and `hardware_model` on the
host. The sequences only a port calls (`OrdinaryTxOwner`'s queued single
attempt, the receive session's `port_*` steps) have no caller; the
compiler's dead-code warnings list them.

The driver's MAC building blocks cover initialization and receive policy,
EDCA, DMA ownership, aggregate formatting, local/TSF time relations,
transmission costs and work receipts. Hardware rate codes come
from the HAL's register-model domains. `mac` groups these primitives beside
the driver's RX/TX/IRQ orchestration; callers use `oer-chips-esp32s31-hal::types`
for HAL values, including `MacLegacyRate`. Portable rate conversions live
in `mac::portable`.

The explicit `test-support` feature enables synthetic completion evidence
and DMA models. The MAC exposes `mac::tx_cost` and `mac::tx_work`, what an
attempt cost on the air and what work it did; which frame goes is the MAC's
queues' choice ([docs/network.md](../../../../../docs/network.md), "Transmit
on the station: the MAC's queues").

## Rules

- **Driver core.** The driver owns all four EDCA queues. Its TX forms are a
  caller-selected single attempt and a fixed-rate `ManagementTxPlan`. The
  caller supplies RTS/CTS or CTS-to-self and its legacy control rate; the
  driver retains no BSS protection or retry ladder. ESP-NOW protocol state
  stays in `oer-protocols-ieee80211-core-extensions-esp-now`, with no dedicated driver TX entry point. A protected
  MPDU is numbered at its first launch; DMA
  is reached only through its owner type, never alongside a quiescence proof.
- **MAC ownership.** The driver coordinates MAC stop, RX draining and the
  radio client's channel retune through owned HAL transactions. The radio
  level latches
  power events. Caller-supplied coexistence intent selects the publication PTI.
- **MAC preparation and start.** The driver takes its block once: `join`,
  `track`, the tracked member's `reset_mac` request, then its own cold
  preparation in its own task, which takes the reset's proof. Cold
  preparation programs configuration on the Wi-Fi partition alone, with the
  common MAC gates closed, and never writes an interrupt bank;
  runtime start requires the
  prepared RX ring's DMA proof, then opens the gates with MAC stop held,
  publishes BASE once, enables the walker and resumes RX, each step after its
  predecessor's proof. Promiscuous policy does not release MAC stop.
- **TX queue collision** uses primary MAC event bit 8. The current station
  mask keeps it disabled; only a queue's `collision_pending` qualifies a TX
  collision detach. HE BSS-color collision uses INT1 bit 12 instead. HE
  initialization enables that separate source, but the open IRQ path has no
  INT1 service; the primary station mask does not suppress it.
- **Timeouts.** Packet-number exhaustion is a typed refusal before
  submission. No retry after a hardware TX timeout.
- **RX storage.** The large-RX profile uses 96 ring buffers and one reserved
  buffer R, each with 4608 backing bytes. Accepted units contain the descriptor
  interval from the software cursor through its first terminal RX_DONE inside
  frozen LAST. Descriptor lengths and ownership grants govern completion and
  recycle; preparation and recycle preserve buffer bytes. A cold cursor at R
  includes R in the first unit until its terminal, while a retained ring NEXT
  starts directly in the ring. SOURCE[HW18_RX_COMMITTED_PREFIX_20261027] in
  `chips/esp32s31/registers/evidence/hw-rx-radio.toml` supports the exact-prefix
  hardware assumption for three measured accepted chains.
- **Protocol RX.** Protected Data is validated through this crate's CCMP
  validator (`mac::rx`), which retains its public header and PN, and removes
  whatever MIC bytes the unit still holds before protocol delivery. The hardware
  itself writes no MIC into a unit it decrypted: the unit holds the header, the
  CCMP header and the plaintext, 8 bytes short of the RX-control tail's length
  less the FCS. SOURCE[HW_RX_CCMP_MIC_CUT_20261118] in
  `chips/esp32s31/registers/evidence/hw-rx-radio.toml`. Integrity is hardware evidence only after
  that validation; replay admission remains in the protocol. The borrowed view
  keeps its original DMA lease and on-air length. Raw monitor normalization
  does not assert cipher integrity.
  The CCMP validators execute from `.hot.text.open_radio_rx_hot` in SRAM on
  every protected protocol RX frame. This validation path has a 1056-byte
  additional SRAM placement budget; other SRAM sections and allocated stacks
  retain their existing limits.
- **GTK replacement.** The station keeps one physical group-key slot
  (`mac::crypto`). A replacement whose new write the hardware rejects
  restores the previous entry; a rollback that fails leaves the slot's
  authority invalid.
- **Station TSF.** An update that would cross 2^64 is refused before any TSF
  write; a beacon whose TSF the station cannot follow is dropped whole.
- **HE service.** The transmit programs carry HE SU attempts and HE SU
  A-MPDUs. The chip MAC and HAL contain Trigger queue primitives; nothing
  here offers a Trigger-response or beamformee service.
- **Host model.** `hardware_model` (feature `test-support`) stands under the
  receive session and the DMA owners where the chip has its registers, so a
  host test runs them joined. Each behaviour is a named rule with its
  evidence, listed in the module; what has none (an abort's end under forced
  CCA, the collision bit, the tail's `rx_state`, the key at capture) is a
  test's input, never a claim about the chip. Passing proves the logic above
  the rules, not the rules.
- **Access point's beacon timestamp.** On a voice-queue management
  transmission of the access point's interface the MAC writes that
  interface's TSF into octets 24..32 of the MPDU on the air, after the queue
  and medium wait; the caller's value is replaced there and stays in the
  buffer (hardware evidence `HW_AP_BEACON_TIMESTAMP_RUN4_20261110`). It was
  observed for beacons and probe responses at DSSS 1 Mb/s; action frames,
  other rates and the exact write moment inside the PPDU are not
  established. The hardware model plays the rule (`AP-TSF`,
  `TX-AP-TIMESTAMP`).

## TX CONTROL bit 22

The HAL's legacy, HT and HE formatter sets queue CONTROL bit 22 on every
prepared publication. This is a known difference from the vendor's
reserved-TXOP-slot adjustment; the bit's physical meaning and effect on air
remain unqualified.

SOURCE: `VENDOR_WIFI_TXOP_DESCRIPTOR_CLASS` in
[`vendor-opaque-identities.toml`](../../../registers/evidence/vendor-opaque-identities.toml),
from pinned `esp32-wifi-lib@f856b5db55cb18e2d1826044430f3f4089e47cf9`.
The vendor adjusts this lane for reserved TXOP slots; frame class or cipher
alone does not determine its final value. This driver does not implement that
adjustment.

## Radio interrupt part

One entry of the image's interrupt table binds `WifiIrqPart` once, at start:
its sources are the MAC bank (`MODEM_WIFI_MAC`), the WDEVPWR bank
(`MODEM_WIFI_PWR`) and the radio doorbell (`FROM_CPU_INTR3`), all routed to
the entry's one line at the `radio` level. Its handler holds the part, with both banks in
their interrupt epoch (`mac::irq::epoch::InterruptEpoch`), for
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
line. A fatal cause returns the MAC module's `mac::irq::Fault::FatalAssertion { raw }`
and keeps both banks quarantined. Closing or activating a quarantined epoch returns the
same fault without clearing it. While Masked (or Closed), a new fatal cause
remains in RAW; detection waits for a later handler entry, which that masked
cause alone does not guarantee.

Staged Wi-Fi images select the `wifi-fault` feature of the chip composition
(`oer-chips-esp32s31-platform-runtime`) and pass the RAW sample to its `wifi_fault::reset`, retaining their interrupt and preempted DMA
owners. That shared module owns one twelve-byte retained record and calls
`fatal::reset_recording`; after the family adoption driver installs the early console,
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

The line's owner asks for a bank change by setting a request bit
(`fetch_or`, Release) and ringing the doorbell; the handler preempts the
task, so a request that no
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
it again. A receive service requests `UnmaskRx` only after the RX frontier
drained with no released buffer left to rearm. Installing a line clears a stale request. A
lease the upper layer returns posts the RX-capacity fact
(`RxReturnNotifier`) in the receive signal, so the next pull rearms the returned descriptors even
when an exhausted walker raises no RX interrupt; the notice touches no ring
state.

## Receive path

The receive session (`rx/session`) services the ring once a call when the
RX interrupt left work, takes each completed unit and stages it in the
staging pool with no copy; chained, oversized, empty and too-long units are
dropped and counted there. A walker that ran out of armed descriptors
raises no receive interrupt, so a returned unit is the only edge left: its
return posts the RX-capacity fact, and the next service rearms the ring.
A received buffer dropped without its recycle or lend leaves its
allocation out of the ring.

One received frame and the owner of its DMA buffer at each step:

1. **The `radio` level.** The radio handler masks RX delivery, acknowledges and raises
   the RX fact in the image's static receive `IrqSignal` of the station's
   executor (`oer_chips_family_platform_interrupt_table::signal`), which pends it and
   touches no waker; the buffer is the DMA engine's.
2. **Actor.** The executor's next round wakes the actor, the signal's
   one waiter; it takes the fact and runs the product service
   (`rx/transaction`), which takes the completed unit
   and stages it: the original buffer becomes a `NetworkRxFrame` lease of
   the staging pool, with no copy. Chained, oversized, empty and too-long
   units are dropped and counted there and never leave the pull.
3. **Actor.** `QueueSink` (`rx/upper.rs`), the service's publisher, marks the
   lease's first handoff (the monotonic time and the prefix's raw MAC
   stamp), counts it and sends it into an affine SPSC queue as deep as the
   pool; a send after a pool credit therefore never refuses. The queue owns
   the lease. The service unmasks RX through the doorbell once drained.
4. **Actor.** `FrameReceiver::next` takes the lease and holds it as a
   `HeldRxFrame`; `normalize` decodes the prefix under `W1_INGRESS` (no
   strict RXEND or dump-length refusal; what they would refuse is counted)
   and counts refusals by reason. The application owns the lease.
5. **Actor.** The MPDU and `portable::rx_meta` on the configured channel go to
   `sta::receive::classify` and `observe_scan`.
6. **Actor.** Dropping the held frame releases the lease and posts the
   RX-capacity fact (a raise of its own executor's receive signal,
   `FromOwnTask`);
   the next service rearms the buffer.

Teardown waits, bounded, until no lease is staged before it stops the
walker, and queued frames are staged: the actor keeps receiving and dropping
frames after it requests the stop, until the exit is published; otherwise
the epoch ends quarantined with `DrainTimeout`.

A trace build (`cfg(oer_trace)`) keeps a stall record
(`rx::counters::stall::RxStall`): the stamps of the descriptors a service
pass observed and of the units it handed on, and on request a snapshot of
the ring, the walker's registers and the session's buffers, read without
writing a register. A product build has neither the record nor the stamps.
