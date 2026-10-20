# IEEE 802.11 lower-MAC port

`oer-ieee80211-port` declares the radio port between portable Wi-Fi MAC
logic and the backend that puts frames on the air, on the owner shape of
[the architecture](../../../docs/architecture.md#radio-port), with the
values its backends' cores take. It is sans-IO: it declares the port and its
values and never waits on it.

## The phase

The port's lifecycle is `oer-radio-port`'s: a backend implements
`LowerMacBackend`, a `PortBackend` whose disabled-only settings are
`LowerMacSetup` (the channel, as a `SupportedChannel`; a backend whose
addresses change also implements `AddressSetup`). An enabled port lends one
running phase (`LowerMacPhase`, module `phase`): one object under `&mut`,
the body's handle over single-producer single-consumer queues to the
backend's driver, which alone owns the hardware state.
Its event stream (`LowerMacEvent`) carries every token an operation took
back; its operations are synchronous and never wait:

- **Tickets and buffers.** A transmission takes a ticket (a place in
  flight and its terminal's place) and the buffers of its frames, each of a
  class sized by the composition (`TxBufferClass`, `ClassFrame`); its
  `TxCompleted` event gives all of them back with `PhaseTypes::Context`,
  chosen by the composition with `type Context: 'id` in `PhaseTypes<'id>`.
  The bound retains install-branded epochs and excludes a reference to a
  machine local to the generic `PortBody::run`. Portable contexts need not
  be `Send`; only a backend crossing execution contexts requires it.
  `PhaseTypes` also owns the single body type and backend fault type for
  the phase and both of its interfaces; interface submission and the event
  stream report that same fault.
  Before submission, `Attempt::admit` or `AggregateAttempt::admit` seals
  context, ticket and every prepared MPDU into one closed `InFlight`.
  The backend receives `InterfaceSubmission` or `InterfaceAggregateSubmission`,
  with execution metadata beside the sealed owner. Queues, publication, withdrawal and quarantine
  retain that whole owner. `TxCompleted::Single` returns its `Completed`;
  `Aggregate` returns the complete ordered subframes. Each subframe owns its
  fresh memory and optional body or its Sent value. There is no independent
  body index array. `TransmitPool` mints tickets and one `TerminalAccess` for the consumer.
  Only consuming `Completed::into_parts` with that installation- and
  pool-specific permission releases owners. Completion alone grants no
  unpacking authority, and raw `InFlight` admission is private.
  Backend execution uses `ExecutionPayload`'s narrow projection; it may lend
  storage needed by the driver but may not expose a replaceable MPDU or
  owner extraction. The model projections keep those fields private, and
  host/RV32 UI pairs reject swapping tickets, contexts or MPDUs, reconstructing
  a terminal through fields, and returning a partial aggregate.
  The common `conformance` suite checks actual returned
  identities and destructor counts before teardown as well as after it.
  Each hardware backend supplies its own fixture for that harness. Context moves unchanged;
  no `Clone` bound, frame inspection or parallel lookup identifies the
  original operation. Bodies remain owned until the same terminal, even
  when the backend copies their octets into DMA memory. An empty pool is the caller's
  own capacity; an occupied queue is the backend's, which keeps the attempt
  until the queue frees.
- **Preparation and submission.** `TxBufferPool::prepare` checks a source
  where its buffer class is concrete and returns a closed `Prepared` value.
  `TxSource::check` first consumes the raw source into `CheckedTxSource`;
  layout errors return the source before entering the backend. `HeaderLayout`
  accepts an `AckHeader`, `NoAckHeader` or group-addressed header; this type
  selects QoS Ack Policy, with no separate flag at encoding. It
  obtains exactly sized arrays with `split_first_chunk_mut`. Each
  `HeaderLength` variant carries an encoder built from an exact wire array;
  encoding assigns the whole `[u8; N]` to its destination. Storage
  shortage, CCMP shortage and body overlap are `LayoutError`; backend
  `PreparationError` contains retransmission identity/kind, aggregate capacity
  and Block Ack window refusals.
  It retains the phase, install, role and key borrow. `ClassSizedFrame`
  carries a portable machine's size proof until the service adds phase-bound
  rates, without rechecking the class. `TxProtection::{Open, Ccmp(NumberedKey { key, packet_number })}` in
  the fresh source is the sole source of the Protected bit. It derives
  `MpduProtection` for the layout; callers cannot change a header's protection
  independently through a public setter. Station TX keys
  are pairwise; AP TX may use pairwise or group keys. The portable
  `MpduLayout` constructor checks CCMP room and tail-body overlap once,
  before any header/body write or admission. The core reserves a PN before
  handoff; cancelled preparation may leave a gap, and a reserved PN is never
  allocated to another MPDU. Fresh aggregate subframes carry `TxNumbering`
  independently; a numbering/protection mismatch returns every owner before
  writing memory.
  `PhaseRate` retains the capabilities, generative installation brand and
  enabled phase loan through NoAck, legacy/A-MPDU rates, retry plans and
  prepared frames. Two installations of the same backend cannot exchange
  those proofs even when their loan lifetimes coincide. Disable and retune
  require releasing the old loan and checking a new rate on the new channel.
  These identities add no stored bytes. The UI cases beside lower-MAC and
  runtime check both misuse and valid use on host and RV32.
  Preparation borrows `TxBody::bytes` once and keeps that view for both the
  length check and the complete tail copy. Re-reading an external body can
  select another slice; even a shorter second view must not leave stale
  payload bytes in an accepted MPDU. The body owner remains with its source.
  A checked header exposes `header_len`, and its MPDU layout exposes
  `mpdu_len`; neither has an empty state. Block Ack request lengths come from
  the MAC wire encoder's `BLOCK_ACK_REQUEST_LEN`.
  A refused preparation returns the original typed source and its owners;
  the caller still holds context and ticket. `submit` takes the already sealed
  operation and its attempt parameters and can refuse only poisoning.
  The queued operation owns backend data, never a borrow of a local machine.
  A retransmission takes the returned `Sent` MPDU and a phase-bound
  `RetransmissionPlan`: header, payload, body and installed-key identity
  remain inside that MPDU. SN and PN stay; only Retry changes in its bytes.
  Rate, retry plan, RTS/CTS, power and the coexistence operation belong to the new attempt.
  Retiring the original installation makes preparation refuse
  `RetransmissionKey`, even if the slot and key material are reused.
  An aggregate builder owns its receiver, TID, role, rate and one protection.
  Its `PhaseAggregate::protection` derives the same typed layout value from
  its retained key, and each subframe produces an `AckHeader`.
  `CheckedAggregate::new` checks the class-specific `SubframeSource` with
  its builder and owns both until `TxBufferPool::push_aggregate` consumes it.
  The proof cannot be transferred to another builder. Fresh subframes have no protection argument, and
  their receiver/TID come from the builder. A Sent subframe retains its
  original installation and must match the builder. Refusal returns both
  the unchanged source and the builder with all earlier subframes.
  A builder using a compressed Block Ack window refuses `BlockAckWindow`
  before publication when a subframe is outside the 64 sequence numbers
  beginning at its first subframe, modulo 4096.
  A successful push returns `NonEmptyAggregate`; only this state can be
  submitted. `AggregateFull` remains a push refusal, and an empty aggregate
  has no submit operation.
  `PhaseAggregate::into_parts` cancels either state without a ticket or TX
  terminal. Cancellation iterates complete MPDUs and does not manufacture an
  admitted aggregate from an empty builder. Sent subframes retain their
  bodies; each fresh subframe returns its own body. `renew` returns memory
  and body explicitly.
- **Management plans.** `ManagementTxPlan` carries a legacy rate derived
  above the driver and a constructor-owned budget: `unicast(rate)` admits
  at most seven attempts (dot11ShortRetryLimit), `broadcast(rate)` one.
  Every attempt keeps the selected rate. The S31 direct W2 path consumes
  this contract without parsing a BSS or using vendor control schedules.
- **Retry plans.** An individual frame carries its `RetryPlan`, a sequence
  of rates with their transmissions and protection, as mac80211's rate
  table. The first stage is mandatory; the tail is a fixed array of
  `RETRY_STAGES - 1` cells whose occupied cells form a prefix, which every
  operation keeps, so mapping and filtering a plan are total and cannot
  exceed the bound. Filtering a plan keeps
  each accepted stage's full budget and order, within the original bound.
  The backend retries the head of its queue by the plan before the
  next attempt of that queue; the completion reports the transmissions made
  and the last rate, for the rate control above the port.
  A single MPDU ends as `SingleOutcome` (`TxOutcome<SingleEnd>`), an
  aggregate as `AggregateOutcome` (`TxOutcome<AggregateEnd>`), so neither
  can carry the other's response. `Air` holds the exact nonzero MPDU count
  together with the last transmitted rate (`u16` covers all four
  255-transmission stages); a delivery and an ACK timeout always carry it,
  while a protection failure, contention exhaustion and lost air carry
  `Option<Air>`, `None` when no MPDU reached air. A collision at a later rate
  preserves the preceding air transmission's count and rate. A delivered
  single MPDU names its response: unsolicited, an ACK, or a BlockAckReq's
  BlockAck with its report. A delivered aggregate always carries its
  BlockAck report. The backend's evidence of the response is its own type:
  `PhaseTypes::AckEvidence` for a single MPDU, which a policy needing the
  ACK's SNR requires as `AckSnr`, and `PhaseTypes::BlockAckEvidence` for an
  aggregate, split by `BlockAckSnr` into a measured sample or none, because
  hardware may accept a BlockAck without a measurement.
  The outcome carries no RSSI. `AirDenied` carries `PhaseTypes::AirDenied`,
  `Infallible` for a backend that cannot tell a coexistence abort apart.
  A hardware timeout with no completion ends as `Indeterminate`;
  the backend does not republish that MPDU, stop the queue, or return its
  tail. `TxStatus` describes only Success, AckTimeout, CtsTimeout, Collision
  and TimedOut observations of a publication. Decoder/accounting faults
  poison the backend and produce no `TxOutcome` or returned-resource event.
  Internal collisions remain driver retries; budget exhaustion is
  `ContentionLimit`. Withdrawals name only GateClosed, KeyRemoved or
  Disabled. Rate selection ignores withdrawals, Indeterminate,
  ContentionLimit and AirDenied (`SingleEnd::rate_sample`). A lifecycle barrier waits for one current publication,
  bounded by `LowerMacBackend::PUBLICATION_BOUND`, not the remaining plan.
  ACK still ends as `Delivered`; a needed retry returns
  `Withdrawn(cause)` with Sent, retaining SN/PN. An unpublished attempt
  returns Fresh. Unknown timeout remains Indeterminate. Per-AC terminals
  precede the barrier confirmation in their original order.
- **MSDU age.** Each VIF's EDCA table stores a typed `oer_time::Duration`
  per AC (default 500 TU); `EdcaParameterSet::from_tu` converts MIB units
  at the record boundary. Admission samples backend `now()` at `submit`,
  once per MSDU and per aggregate member. Every publication and retry
  checks age, including re-formation of returned unpublished members.
  Hardware-owned attempts finish independently of age. `Expired { msdus }`
  returns all original owners; bit `i` identifies expired aggregate member
  `i` (bit zero for a single). Live members retain their admission samples
  for re-formation. Renewal for a new MSDU clears the old sample through
  `TxBufferMemory::renew_msdu`. Expiry is counted separately and supplies no
  rate sample. The upper-MAC BA originator produces a BlockAckReq when an
  active window's head expires; the backend generates no BAR.
- **Order.** Every operation reaches the hardware in the order the caller
  made it: an attempt that names a key is published after that key's
  install.
- **Keys and agreements.** A key holds a slot of its interface's own kind;
  its removal ends with `KeyRemoved`, once no prepared or published attempt names it. A
  receive Block Ack agreement is a token that holds its slot, replaced or
  removed by value. Its slot holds desired state: repeated add/replace/remove
  writes coalesce to the latest value before the driver's next command.
  They occupy no command slots. Their capacity is fixed by the types of the
  slots themselves, known at compile time. A key slot (`PairwiseKeySlot`,
  `GroupKeySlot`) names its `AssociationPool<R, P, G>` through the sealed
  `AssociationSlot`, and `KeyTypes::Pool` must be that pool (`KeyPool`'s
  `PAIRWISE_SLOTS` and `GROUP_SLOTS`). A receive Block Ack slot is a
  `BlockAckSlot<'id, K, N>` that only the pool `BlockAckSlots<K, N>` mints,
  and `PhaseTypes::BlockAckPool` must be that pool (`RxBlockAckPool::LEN`).
  A declared capacity other than the minted one does not compile, the
  capabilities carry no copy of these counts, and running out is a missing
  slot token, never a comparison. The command bound is tickets + twice the
  pairwise slots + one GTK permission per interface + configure permissions
  + TSF permissions + two gate commands. The terminal bound is tickets +
  pairwise slots + GTK permissions + gate closing + configure permissions
  + TSF permissions; the event queue additionally reserves RX buffers.
  `PhaseTypes` exposes these bounds. `InterfaceCommandPool` derives both of
  its contributions from the same constants used to mint its slots.
- **Interface commands.** `Mint` gives each interface one affine
  `ConfigurePermission` and one `TsfPermission`, branded by installation,
  role and pool path. `configure(permission, receive)` enters the FIFO;
  `Configured` returns the same permission with its opaque context.
  The station's connection epoch travels in that context, so the service
  does not match a role against a separate pending-request table. A second
  configure is impossible until the first terminal returns. Configuration
  withdraws held attempts of its interface and waits for its published
  attempts; another interface's publication does not block it.
  `sample_tsf(permission)` likewise returns its permission in `TsfSampled`.
  Sampling belongs to the interface whose clock `BeaconTimingPhase` reads.
  Disable drains these command terminals with their owners.
- **Settings.** EDCA, BSS color, a TBTT plan, monitor reception and Block Ack
  desired state take effect before the next publication; the latest value
  of each wins. They consume no FIFO command credit. Each VIF keeps its
  complete EDCA table: AIFSN, ECWmin, ECWmax, TXOP limit and MSDU lifetime.
  `SupportedEdca` refuses unsupported records; `set_edca` applies them
  exactly. The head frame's VIF selects the parameters of its AC queue's
  single contention state. Before a BSS is known, the standard non-AP
  defaults apply (IEEE 802.11-2020 9.4.2.28, Table 9-155), with 500 TU
  lifetimes. Local lifetimes are separate from the on-air WMM element.
  TXOP limits are retained in 32-us units; an access sends one frame.
- **Gate evidence.** The install includes one `GatePool` in its declared pool set.
  Its private `GatePlace` starts inside `GateOpen`
  and retains one `GateIssuer` in the backend. Closing moves that place to
  the driver; only after the publication barrier does the issuer transform
  it into `GateClosed` for the event. Opening consumes that evidence and
  returns the same place as `GateOpen`. There are no independent open and
  closed slots. The arbiter holds these exact branded states; the body can
  neither construct closed evidence, copy the place, nor keep both states.
- **Reception.** A received frame's buffer goes back through `recycle`; a
  lagging body loses unsolicited frames as `EventsLost`, never a terminal.

`oer_ieee80211_wire::ccmp::Ccmp128Key` is the shared affine, zeroizing owner
of installation material. RSN moves this owner into installation. Its
bytes are private; hardware programming and software CCMP borrow them.
It has no `Clone`, `Copy`, equality or owned byte extraction, and Debug is
redacted. The model moves that same owner through its command FIFO into
the key table, where removal or teardown zeroizes it independently of any
surviving slot identity. Quarantine intentionally retains the key without
calling `Drop`, until chip reset.

An `AssociationPool` mints one start permission and the complete PTK/GTK
bank of its association domain. Beginning consumes that permission and fixes
`Spp::{Negotiated, NotNegotiated}`. Every key installation borrows this owner
and consumes a slot from the same installation, role and pool path. Keys
cannot choose independent SPP modes. The backend applies the association's
mode before publishing its first key; its `SppScope` capability declares
whether domains occupy whole interfaces or individual associations.

Pairwise removal carries the association owner through its `KeyRemoved`
terminal. The station first removes its group keys, then the PTK; spare
unused slots remain owned throughout. Only the exact complete `VacantKeyBank`
can enter `end_association(owner, bank)`, which synchronously returns a
`VacantAssociation` for the next exchange. No command or terminal is added.
A partial bank or quarantined key cannot release the association permission.
On an AP, carrying the owner through a pairwise removal temporarily defers
new key installations until that terminal returns; existing keys retain
their immutable mode and admitted transmissions proceed normally.

Each interface receives one affine `KeyTypes::GroupPermission`, minted once
with the installation and branded to that port and interface. Every GTK
install, replacement, selection and removal consumes it and reserves a FIFO
terminal. The synchronous call transfers resources; `GroupKeyInstalled`,
`GroupKeyReplaced` or `GroupKeySelected` alone proves success and returns the
installed owner and permission. `KeyRemoved` returns the vacant slot and
permission. There is no Busy result or timer retry for admission.

Exactly one installation per association/wire KeyID decrypts at a time;
other IDs may stay active. Each successful GTK terminal is a real RX cut:
all delivered old-installation frames precede it, including facts retained
in hardware, DMA, ISR and software queues; selected-installation frames
follow it. The cut is finite under continuous ingress. The backend supplies
completion and deadline wakeups and declares `GROUP_COMMAND_BOUND` from
admission through terminal or Poisoned. Normal disable completes accepted
GTK operations within the same bound. The API exposes no cancellation;
uncertain operations retain their owners and permission for teardown.

`select_group_key` consumes a concrete retained installed owner. It neither
writes material nor creates an installation nor resets counters, including
when already current. A higher layer retains each saved key's replay owner
and exchanges it at the terminal. The port knows no replay state.
`KeyTypes::GroupSlotIdentity` correlates pending actions with their terminal;
`Identity` still names the actual material installation used for TX.

The model covers retained same-ID selection and delayed ingress through the
shared conformance harness. An evicted key is no longer installed and
cannot be selected from a RAM copy. Physical slot counts belong to backend
pools; each hardware backend must prove its own RX boundary.

The install mints one `Slot<ConnectionEpochKind>` for Station's epoch source.
This slot kind belongs to the lower-MAC contract; the backend neither depends
on the station's epoch implementation nor creates connection identities.

A frame's kind (`TxFrame`) fixes its response: an individual frame (an
`AckHeader` with an `IndividualAddress` receiver, any data or management
frame but Action No Ack), an individual frame without an ACK (a
`NoAckHeader` at a `SupportedNoAck` rate), a group frame or a BlockAckReq
(at `LegacyPhaseRate`s). The interfaces' types are their roles
(`StationPhase`, `AccessPointPhase`), each with its own receive rules
(`StationReceive`, `SupportedFilter` of the role).

The module `owner`'s table maps every refusal of the old lower-MAC port to
its type, and lists the world's outcomes that stay. Compile-fail tests in
`owner` and `phase` pin each rule a type carries.

## Optional features

Extension traits of the phase: `AmpduPhase` (one attempt sends one A-MPDU,
each subframe in a buffer of its own), `BeaconTimingPhase` (an interface's
TSF, its samples and its TBTT plan with the wake lead and the beacon window
the `BeaconWindow` it asks the air for as `Operation::BeaconWindow`) and `MonitorPhase`. Their values (`AmpduCapabilities`,
`TbttEvent`, `TsfSample`, `TsfRelation`, `VifTsf`, `MonitorCapabilities`)
are in `extensions`.

Parametric limits of what a backend has are its capabilities
(`LowerMacCapabilities`), and a value outside them has no checked type. Why
a backend lacks a feature or a value (hardware absent, glue not written,
vendor knowledge not recovered, policy decision pending) is recorded in its
documentation, not in code.

The PHY and channel values come from
[`oer-ieee80211-wire`](../wire/src/phy.rs): `Channel` (2.4 GHz and 5 GHz with
20 MHz and 40 MHz geometry) and `PhyRate` (non-HT, HT and HE SU).
`RxEvidence` and `RxCryptoStatus` describe receive provenance. Backend-facing
service values, including `MacTxResult`, `MacRxMetadata`, `MacResourceLimits`,
configuration, monitor admission and `EdcaContention`, live in this port.
`MacOperationOwnership::hardware_services` turns a service description into
`HardwareServices`. Association `PhyMode` and `Preference` also belong here;
wire encoding obtains selected capability elements through `AssociationPhy`.

## Above the port

What a backend does not report in its `HardwareServices` is software above
the port, written once in portable packages for every backend, with one
exception: the backend draws contention for its hardware queues. The core
allocates protected MPDU packet numbers and the backend formats the supplied
PN with the installed key ID.

| Work | Owner |
| --- | --- |
| The retry plan of an MPDU and the rate control that chooses it | `oer-ieee80211-upper-mac` |
| Which A-MPDU subframes the next aggregate resends after a `BlockAckReport` (`BlockAckReport::acknowledges`), the BlockAckReq, aging | `oer-ieee80211-upper-mac` `ampdu` |
| RTS/CTS or CTS-to-self and the control-frame rate | `oer-ieee80211-upper-mac` `protection` |
| Transmit packet-number allocation and receive replay | `oer-ieee80211-upper-mac` `ccmp` |
| Receive Block Ack reordering | `oer-ieee80211-upper-mac` `block_ack::reorder` |
| The station's lifecycle, from its scan to its disconnect | `oer-ieee80211-sta` `machine` |

Limits, rate ladders and duration estimates in which implementations differ
are parameters of those algorithms; the Espressif values recovered from the
vendor stack are the family package `oer-espressif-ieee80211-policy`.

## Implementers

The host model in `oer-ieee80211-model` implements the phase with
composition-selected context types. It returns queued operations on gate
close and normal disable, including queues held by the model air. On
an ordinary fault it returns admitted terminals before `EventEnd::Drained`;
the service verifies every outstanding identity. `EventEnd::Quarantined`
instead ends through fail-stop without another disable/drain. Undelivered
owners stay in permanent quarantine even after its receipt is dropped.
Hardware backends provide the same resource and terminal contract.

`LowerMacEvent::Terminal(LowerMacTerminal)` carries TX, key, configuration,
gate and requested clock completions in the normal FIFO. Disable returns
only `LowerMacTerminal`; RX and TBTT are excluded by type. The backend
recycles unread RX itself and reports the coalesced loss after re-enable.
StationService therefore needs no retained-RX array or receive-capacity
const parameter for disable.


Station group-key slots are fungible, affine resources. The backend stores
and returns each raw slot with its installed key; it carries no station-cell
wrapper. Station returns removed slots to its declared free pool. Its full
pool proves that no group install or removal remains outstanding. Pending
installation tokens carry the protocol key ID and connection epoch.


`backend::BackendTxProfile` is selected statically by the image/runtime. Its
associated retry owner and functions expose core algorithms without a trait
object or a stored profile. Immutable rate records, retry/protection values
and completion facts cross this boundary; rate ladders and vendor tables
remain in the core profile. `backend_aggregate` carries detached BlockAck
facts and aggregate retry commands. `espressif` contains LR and ESP-NOW PHY
requests and static admitted-packet handoffs; ESP-NOW peer/protocol state
remains in softmac.


## Shared backend conformance

The `test-support` feature exposes `conformance` in this crate. Backend tests
implement its ownership, publication-barrier, configuration, GTK and outcome
harness traits; they need no dependency on `oer-ieee80211-runtime`. The model
in `oer-ieee80211-model` runs the same runners with virtual time. Traces contain observations
from actual submitted/returned owners, publication observers and event streams;
expected identities cannot substitute for observations.

Each case exposes `execution()`: `Hardware` or `ModelOnly(reason)`. Runners take
`Environment::Model` to exercise everything, or `Environment::Hardware` to
exclude injected conditions explicitly. Their `Report` counts verified cases
and model-only exclusions separately. The physical fixtures still supply real
ACK/BlockAck evidence and actual backend progress; classification alone does
not prove a hardware implementation.

| Suite | Hardware cases | Model-only cases |
| --- | --- | --- |
| Ownership (`check`) | Complete, close gate, disable, admission refusal | Poisoned backend |
| Publication (`check_barriers`) | Gate with ACK, Sent retry, key removal | Lost hardware completion |
| Configuration (`interface_commands::check`) | Both interface orders, running/queued/posted disable | None |
| Outcomes (`outcomes::check`) | Single ACK, aggregate BlockAck, single/aggregate NoResponse | Single/aggregate Collision, AirDenied, HardwareTimeout |
| GTK (`group_keys::check`) | Disable before/after each of four mutations, published-key removal | Four independently delayed-ingress RX cuts, four stalled-command expiries |
| EDCA (`edca::check`) | Two VIFs with different tables, every AC | None |
| MSDU age (`msdu_age::check`) | None | Before publication, between retries, re-formation with a core BAR, delivery before head expiry, hardware-owned completion |

Hardware ACK uses a reachable peer and NoResponse an absent address. Collision,
AirDenied and HardwareTimeout require model injection. The GTK receive-cut
cases independently inject old-key facts at hardware, DMA, interrupt and
software stages; ordinary RF traffic does not establish those exact cuts.
Command expiry and fail-stop also require explicit faults. Hardware cases
that intersect a held publication or a queued command require cooperative
backend progression and observation at the specified boundaries.

The feature adds no allocator or executor dependency. Harness associated types
select trace storage through `AsRef<[T]>`: host fixtures use `Vec`, and an image
can use arrays or bounded collections. `BarrierHarness` also selects frame and
retry storage. GTK checks take an independent `oer_time::Timer` to bound the
whole fixture, including a backend that never delivers its terminal; model
fixtures pass a virtual timer. A standalone consumer in `tests/conformance.rs`
exercises runner selection and negative verifiers with arrays and `heapless`.
