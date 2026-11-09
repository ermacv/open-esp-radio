# oer-chips-esp32s31-radio-ieee80211-backend

The ESP32-S31 Wi-Fi driver: device composition, the data port the station's
datapath actor owns (`port::Esp32s31Port`), DMA rings, keys and the path
from the P3 interrupt to the actor's task. System context: [architecture](../../../../../docs/architecture.md).

The driver's MAC building blocks cover initialization and receive policy,
EDCA, DMA ownership, aggregate formatting, local/TSF time relations,
transmission costs and work receipts. Hardware rate codes come
from the HAL's register-model domains. `mac` groups these primitives beside
the driver's RX/TX/IRQ orchestration; callers use `oer-chips-esp32s31-hal::types`
for HAL values, including `MacLegacyRate`. Portable rate conversions live
in `mac::portable`.

The explicit `test-support` feature enables synthetic completion evidence
and DMA models. Portable peer airtime fairness belongs to
[`oer-protocols-ieee80211-core-upper-mac::airtime`](../../../../../protocols/ieee80211/core/upper-mac/README.md).
The MAC exposes `mac::tx_cost` and `mac::tx_work`; it does not select peers
or own the fairness ledger. Tests under `src/mac/airtime_tests` combine the
upper-MAC scheduler with work receipts and network TX queues, using only
development dependencies on the upper MAC and `oer-portable-embassy-net`.

## Rules

- **Driver core.** The driver owns all four EDCA queues. Its TX forms are a
  caller-selected single attempt and a fixed-rate `ManagementTxPlan`. The
  caller supplies RTS/CTS or CTS-to-self and its legacy control rate; the
  driver retains no BSS protection or retry ladder. ESP-NOW protocol state
  stays in `oer-protocols-ieee80211-core-extensions-esp-now`, with no dedicated driver TX entry point. A protected
  MPDU is numbered at its first launch; DMA
  is reached only through its owner type, never alongside a quiescence proof.
- **MAC ownership.** The driver coordinates MAC stop, RX draining and the
  radio client's channel retune through owned HAL transactions. P3 latches
  power events; the port tracks station TBTT deadlines from the installed schedule.
  Caller-supplied coexistence intent selects the publication PTI.
- **MAC preparation and start.** Cold preparation programs configuration
  with the common MAC gates closed and never writes an interrupt bank;
  runtime start requires the
  prepared RX ring's DMA proof, then opens the gates with MAC stop held,
  publishes BASE once, enables the walker and resumes RX, each step after its
  predecessor's proof. Promiscuous policy does not release MAC stop.
- **TX queue collision** uses primary MAC event bit 8. The current station
  mask keeps it disabled; only a queue's `collision_pending` qualifies a TX
  collision detach. HE BSS-color collision uses INT1 bit 12 instead. HE
  initialization enables that separate source, but the open IRQ path has no
  INT1 service; the primary station mask does not suppress it.
- **Keys and timeouts.** After an unconfirmed stop the port keeps every
  owner and the installation stays quarantined until the image resets; a
  retired installation's device masks the ring's interrupts and erases its
  keys. Packet-number exhaustion is a typed refusal before submission. No
  retry after a hardware TX timeout.
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
- **GTK replacement.** The station keeps one physical group-key slot. A
  replacement the key table refuses before it changes, or whose new write
  the hardware rejects and whose rollback restores the previous entry,
  returns its command whole beside `Fault::GroupReplacement` (`Refused`,
  `RolledBack`): the previous key stands with its identity, and the core
  runs on. A replacement whose rollback fails keeps the command's owners in
  the port with both causes (`RollbackFailed`); the previous key is not
  live, and the port does not retire. The stopped port's service clears the
  entry once and reads it back: cleared, the key's removal returns the slot
  and the permission (`KeyRemoved`); not cleared, nothing is freed and the
  fault is `GroupEntryUnconfirmed`. The receive handle retains zeroizing
  rollback material until removal or reset.
- **Station TSF.** An update that would cross 2^64 is refused before any TSF
  write; a beacon whose TSF the station cannot follow is dropped whole.
- **HE service.** The lower-MAC port accepts caller-selected HE SU attempts,
  and the station's data arenas publish HE SU A-MPDUs. It exposes no
  Trigger-response or beamformee service. The chip MAC and HAL contain
  Trigger queue primitives; their presence does not advertise those
  services through the port.
- **Host model.** `hardware_model` (feature `test-support`) stands under the
  real port, device, receive session and DMA owners, where the chip has its
  registers, so a host test runs them joined; the station facade's `joined`
  tests run a socket through them. Each behaviour is a named rule with its
  evidence, listed in the module; what has none (an abort's end under forced
  CCA, the collision bit, the tail's `rx_state`, the key at capture) is a
  test's input, never a claim about the chip. Passing proves the logic above
  the rules, not the rules.
- **Host tests.** The shared data-port cases
  (`oer-protocols-ieee80211-testing-conformance::data_port`) run against
  `Esp32s31Port` over the host device model, beside the port's own tests in
  `lower_mac/tests/port/data_port` (transmit and barrier order, data
  arenas, stop, reinstall and release, keys, receive cuts and receive
  room). The model holds a queue's completions, injects a TX-timeout edge
  and a collision, answers each A-MPDU's BlockAck, rejects key writes,
  decrypts air frames at capture against its key table and drains
  captured hardware, DMA and interrupt frames through the RX cut. A
  publication is identified by its program's PSDU length: every host DMA
  allocation has the same model descriptor address. The shared
  access-point cases (`access_point::check`) run against the same port and
  model (`lower_mac/tests/port/data_port/access_point`): both roles on one
  port, each role's configuration barrier, beacons and their expiry, stale
  tokens, keys and stops. Not run: air denial (no device model).
- **Access point.** The port implements `AccessPointPort` with its own
  commands: a configuration withdraws the access point's queued attempts,
  holds while one of its own is on the air or the core configures, and
  restarts or stops the AP TSF; while the core configures the access
  point, the station's attempts still publish (`ConfiguringView::other_interface`).
  Its group key installs and removes through the same RX cut as the
  station's. A beacon queued on VO past its deadline ends `Expired` with
  its permission (`expire_beacons`). No Timestamp rule is declared
  (`AccessPointServices::beacon_timestamp` is `None`), so no `BeaconStamp`
  exists for the S31 and the port takes no beacon: the vendor's beacon path
  writes the MAC's local-time count, not the access point's TSF, into the
  frame in software, no timestamp-insertion control is known, and whether
  the hardware replaces the field at transmission is not established. The
  access point's runner writes the Timestamp itself (`Caller`): over this
  port it ends `ApEnd::Unsupported` before the radio starts, and the S31
  composition's start refuses `StartRefused::Unsupported` with its
  settings before anything starts. Should the field turn out to be the
  transmitter's (`Transmitter`), the runner needs that rule's support
  before the S31 can serve an access point. On the S31 the access-point
  suite's beacon cases end `Fixture`, which is no coverage; its two stops
  run over ordinary access-point frames (`StopsAloneFrames`,
  `SharedStopFrames`).
- **Publication observations.** `Esp32s31Port`'s last parameter `O` is
  the port's observation receiver (`port::Observations`, an
  `oer_portable_events::EventSink<ContentionDraw>`). It arrives in `PortParts`, is
  stored once in the driver from preparation to retirement, passes to the
  next installation on a reinstall and is lent to each contention draw, where the driver selects
  the head's VIF record. `ContentionDraw` holds values only: role, access
  category, the complete record, the current window and the drawn backoff.
  The event is built only for an enabled receiver; images pass `()`. The
  EDCA host suite checks each draw separately against the AIFSN and backoff
  of the legacy program the driver handed the hardware.
- **Conformance.** The shared conformance cases run only as the host tests
  above; no image runs them on hardware. The GTK rollback case rejects the
  new key write in the host device model, and restoration programs that
  model's key table.

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
the entry's one P3 line. Its handler holds the part, with both banks in
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

The port asks for a bank change by setting a request bit
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
it again. The port's receive pull requests `UnmaskRx` only after the RX frontier drained with no
released buffer left to rearm. Installing a line clears a stale request. A
lease the upper layer returns posts the RX-capacity fact
(`RxReturnNotifier`) in the receive signal, so the next pull rearms the returned descriptors even
when an exhausted walker raises no RX interrupt; the notice touches no ring
state.

## Receive path

The datapath actor takes received units in its own turn through
`DataPort::receive`: a staged unit, or the ring serviced once when the RX
interrupt left work, one unit per call result and at most eight refused
units per call. The call takes the turn's instant, never reads the clock
and never wakes its caller: ring work left now is `RxPull::Later(now)`,
work waiting for its continuation `RxPull::Later(at)`, which the port's
deadline covers. A unit the actor recycles goes straight back to the ring,
and its lends come off its count at the next pull. The MAC's receive facts
(a received unit, a returned staging credit) travel in their own signal,
which the port's facts poll (`DataPort::poll_facts`) beside the transmit
and TBTT facts, all in the actor's task. The facts call only registers
and looks: the pull that serves the facts takes them. A fact raised during
a turn therefore still wakes the actor at the executor's next round,
whether or not the actor reposts after the facts that followed its turn.
The RX interrupt stays masked until a pull drained the ring, so that wake
is the masked ring's only edge. While a group mutation's receive cut or a
stop holds the ring, the pull takes nothing; a cut's units reach the actor
through its sink ahead of the mutation's terminal.

Every unit the network returns is a receive fact at the actor's next facts,
whose next pull rearms the ring: a walker that ran out of armed
descriptors raises no receive interrupt, so the return is the only edge
left.

When the actor holds `RX_HELD` units, a pull disarms only the RX
continuation timer and keeps the pending RX work and buffer owners; it
services the ring again once the actor returned or lent a unit.

The actor holds at most `RX_HELD` (16) received units besides those it
lent the network: the units it took and the MPDUs its receive Block Ack
agreements keep, a 16-MPDU reorder window. The units outside the ring come
from one stage pool; the network may hold all of it but `RX_HELD` and the
admission's critical reserve (`port::lent_rx`). The admission discards bulk
input, which the hardware already acknowledged, once only the reserve is
left, so this split keeps a full reorder window's hole-closing MPDU staged.
A received buffer dropped without its recycle or lend leaves its allocation
out of the ring, and the port answers every later call with
`Fault::ReceiveLost`.

One received frame and the owner of its DMA buffer at each step:

1. **P3.** The radio handler masks RX delivery, acknowledges and raises
   the RX fact in the image's static receive `IrqSignal` of the station's
   executor (`oer_chips_family_platform_interrupt_table::signal`), which pends it and
   touches no waker; the buffer is the DMA engine's.
2. **Actor.** The executor's next round wakes the actor, the signal's
   one waiter; the port's receive pull takes the fact and runs the
   product service (`rx/transaction`), which takes the completed unit
   and stages it: the original buffer becomes a `NetworkRxFrame` lease of
   the staging pool, with no copy. Chained, oversized, empty and too-long
   units are dropped and counted there and never leave the pull.
3. **Actor.** `QueueSink` (`rx/upper.rs`), the pull's publisher, marks the
   lease's first handoff (the monotonic time and the prefix's raw MAC
   stamp), counts it and sends it into an affine SPSC queue as deep as the
   pool; a send after a pool credit therefore never refuses. The queue owns
   the lease. The pull unmasks RX through the doorbell once drained.
4. **Actor.** `FrameReceiver::next` takes the lease and holds it as a
   `HeldRxFrame`; `normalize` decodes the prefix under `W1_INGRESS` (no
   strict RXEND or dump-length refusal; what they would refuse is counted)
   and counts refusals by reason. The application owns the lease.
5. **Actor.** The MPDU and `portable::rx_meta` on the configured channel go to
   `sta::receive::classify` and `observe_scan`.
6. **Actor.** Dropping the held frame releases the lease and posts the
   RX-capacity fact (a raise of its own executor's receive signal,
   `FromOwnTask`);
   the port's next pull rearms the buffer.

Only the actor's task calls the port, so nothing else borrows the device or
the driver's state. Teardown waits, bounded, until no lease is staged
before it stops the walker, and queued frames are staged: the actor keeps
receiving and dropping frames after it requests the stop, until the exit
is published; otherwise the epoch ends quarantined with `DrainTimeout`.

A trace build (`cfg(oer_trace)`) keeps a stall record beside the device
(`rx::counters::stall::RxStall`, lent through `RadioParts::stall`; the
station facade holds one static). The device stamps two boundaries with the
turn's instant: the descriptors a service pass observed (DMA) and the units
it hands to the port (driver). A diagnostic reader on the device's hart asks
for a snapshot (`RxStall::request`), which wakes the actor's task through
the waker the device registers at every `poll_rx_work`. At the first port
facts after the request, before the turn's receive pull takes the receive
facts, the device reads, without writing a register, moving a
descriptor or taking a fact: both stamps, its own ring work, the walker's
registers, the ring's view, every descriptor's `OWNER` and `RX_DONE`, the
session's buffers and the software side of the interrupt line, and the
MAC statistics bank beside its last periodic sample. The request counts as
a receive fact, so the turn runs, and leaves `rx_pending` as it was: the
turn's pull serves only what the device already knew of. At each facts
after the snapshot the device records the stamps again, and for the last
time once the turn's receive pull ran. A product build has neither the
record nor the stamps.

## Lower-MAC core

The driver core `LowerMacCore` lives in [`src/lower_mac.rs`](src/lower_mac.rs).
The data port [`src/port/engines.rs`](src/port/engines.rs) (`Esp32s31Port`)
implements the contract's `DataPort` over that core for the station: it
owns the driver's state, the device and the settings by value, and the
datapath actor's task alone calls it, each call with the turn's instant.
It never reads the clock in a call and never wakes its caller; what must
run again is in the returned value or in its cached deadline, which every
call that changes a queue recomputes. A barrier command (a configuration
while an arena is on the air, a removal before its drain, a group
mutation's receive cut) comes back `Held` for the actor to retry; a stop
forgets its barrier, the stopped port refuses it `NotEnabled`, and after a
restart it runs again under a bound measured from then. A broken
contract of the port's own state is a named `Fault` that keeps its
observations; the port keeps every owner it holds and answers each later
call with the same fault. Its command issuer (`Esp32s31Commands`) mints
each installation's key identities for the station's lifecycle. One
installation (`Esp32s31Port::install`) takes the first minted
installation; after a confirmed stop, `reinstall` mints the next one in
place on the same device, driver state and allocations, and `release`
hands the device back. The P3 handlers own only their interrupt epoch and
static typed signals. The task-side `RadioDevice` retains the cooperative
registers, PHY actor transaction and RX ring. No ISR accesses its
task-only `RefCell` state. It names what it needs of the stopped radio's
owner (`Frontier`) and of the registers (`ReceiveRegisters`); on the chip
those are `WifiStopped` and `RadioAccess`, on a host the model below.

Eight shared TX tickets feed four hardware queues; an MPDU takes one, and
the station's two data arenas hold one each. Every MPDU is written into one
buffer of the transmit pool (`TX_BUFFERS`) placed in the DMA window; the
network's data frames come in memory of the same pool, which the arenas
hold. Descriptors are separate publication metadata: an MPDU's buffer is
attached to its queue's ordinary descriptor for the publication, and each
data arena retains its frames in one of the two descriptor chains
(`AMPDU_CHAINS`). Each queue executes an MPDU's supplied retry plan.
Timeout aborts of MPDUs and of the arena on the air force CCA one at a
time: the queue's timeout edge begins one, and so does its publication
watchdog without that edge, since a busy channel can hold a queue past its
programmed timeout. The vendor's stop sequence then decides by VALID: a
valid queue is disabled and the attempt ends; a clear one took its PPDU, so
the owners stay with it until its completion, and a second watchdog without
one records the fault and resets. Gate, configuration, key and disable barriers wait for published
MPDUs and arenas and return the original operation owners through
terminals. A reinstall after a confirmed stop reuses the stopped owners for
the next installation; uncertainty keeps the DMA allocations quarantined. RX buffers require
explicit recycling. GTK changes freeze ingress and drain preceding RX
before returning their permission. An erased key cannot authorize a
returned MPDU's retransmission.

`lower_mac::arena` implements the port's `TxArena` over the two chains
(`Esp32s31TxArena`, built from the chains and two tickets; `ArenaPort` lends
it the core, hardware, free queues, pairwise key, sink and contention
observer for one step). Intake moves the network's buffer into the chain's
next retention slot, hands the writer the region after the eight-byte
prefix and binds the descriptor word once, its MIC room from the MPDU's
Protected bit; every publication, a retry included, relinks the bound
words. Publication refuses a Protected bit the encryption contradicts and
an A-MPDU past its rate's octet ceiling or the chain's slots, admits the
pairwise key for an encrypted arena and programs an HT/HE A-MPDU of every
frame or position 0 alone as a legacy, HT or HE single MPDU. A single MPDU
runs its retry plan as the driver runs an MPDU's: each completion without
an ACK, a collision or a protection failure takes a try of the current
stage, and the frame goes again at the stage's rate with a grown contention
window and a new watchdog, until an ACK or the plan's end; its end reports
every transmission that reached the air. A completion without an ACK sets
the frame's Retry bit; a collision or a protection failure leaves the bit
as it is. Completion maps
a valid BlockAck, an ACK, an ACK or CTS timeout, a collision detach and a
timeout abort onto `AttemptEnd`; a collision or abort detach keeps every
buffer. Retention returns the frames not kept in position order, compacts
the kept ones with their bound words and sets the Retry bit only on kept
frames the publication sent. The key right stays with the arena until no
frame it encrypted is published or retained. Its one deadline is the
publication watchdog.

The driver builds the arenas at preparation and takes their chains and
tickets back at retirement. The retire checks require both idle and empty
and holding no key right; otherwise the stopped port refuses reinstallation
and release as not retired and keeps its installation. A busy descriptor
found at retirement refuses reinstallation with a resource fault. The actor reaches them through `DataPort::arena`
(`port::Esp32s31Arena`), lent for its turn: it fills, publishes, withdraws
and settles them, and a publication is refused as busy while the gate is
closed or closing, a configuration or a barrier command is pending, or the
port stops. The actor offers a completion edge to the arena on the air
first (`DataPort::service_arena`), so the next arena goes out ahead of the
port's other transmit work; the port serves everything else, and the arena
on any other edge and at its deadline, in its transmit service
(`DataPort::service_tx`). Either returns the arena's end to the actor, whose
settlement returns its frames' memory. An HT aggregate whose values match
the last one's takes that one's program with a backoff of its own; a command
or a changed setting forgets the program. The arena on the air holds its access category's queue:
an MPDU of that category waits for the arena's end. A stop serves a
published arena to its end (its watchdog's timeout abort when the
hardware gives no completion) before it disables the core. A station
pairwise key removal waits while an arena holds a key right.

A key removal whose key still has an admitted use when its command runs
is done for its command (`Done`): the core clears the key once its last
use returns and keeps the receipt in the removal's core place (every
sink refuses receipts), and the port keeps the command's rights in the
key's own table place, which the removal emptied (`KeyPlace::Removing`):
no second removal can name that place while they wait there, and no
installation reuses it. Each later service, a stopped port's
included, takes each finished receipt from its own place and ends that
removal once with those rights (`deliver_removals`); a receipt of another
key never answers the command at hand. A port with a waiting removal does
not retire.

A removal names its slot kind in its types: the core keeps each removal
in a place of its key's slot kind (`RemovalPlace<S>`, one array per kind,
as long as that kind's pool) and its receipt is of that kind
(`KeyRemoved<S>`); the port's rights of each kind (`RemovalRights`) take
only a receipt of their own kind. A key of another kind than the rights
is refused before anything is taken (`RemovalMismatch`, the command back
whole).

A terminal or a cut unit the caller's sink refuses stays with the port
(`port/unconsumed.rs`): a terminal in the place of its token (its ticket,
the gate, its interface's configure permission, its pairwise key slot or
its group permission), a unit in one of `RX_HELD` places, one relative
sequence over both. While the radio has not confirmed a stop the port
offers them again, oldest first, at the start of `service_tx`, `submit`,
`command` and `access_point_command`, and a newer terminal waits behind
them; after a confirmed stop only `take_unconsumed` offers them. Nothing
the port keeps makes its deadline due or raises a fact, and a port that
keeps one does not retire. A taken or missing place (a token that was not
unique) is the contract fault `UnconsumedPlace`.

A station group key mutation and an access point's are commands of their
own role (`StationGroupCommand`, `AccessPointGroupCommand`). A step of
the driver that refuses, and a replacement the hardware rolled back,
return the command whole: the engine converts it back to the caller's
form and the frozen receive cut resumes. Only a replacement whose
rollback failed keeps its owners in the port (`retained_group`) until the
stopped port's cleanup above.

The port exposes ordinary MPDUs, the station's data arenas (with their
A-MPDU rates) and RX BlockAck agreements. It does not expose beacon
timing, monitor or air reservation.
Its publication watchdog is 2 s; its command bound is 2.5 s, and a command
held behind published frames counts from the last one that held it.
The operation mapping and portable limits are in
[the port's README](../../../../../protocols/ieee80211/contracts/port/README.md#implementers).
