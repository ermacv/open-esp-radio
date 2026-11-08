# IEEE 802.11 lower-MAC port

`oer-protocols-ieee80211-contracts-port` declares the radio port between portable Wi-Fi MAC
logic and the backend that puts frames on the air, as
[the architecture](../../../../docs/architecture.md#radio-port) describes it. It holds only
the interaction with the peripheral and the representation of the data that
crosses it: typestate wrappers, outcomes and facts. Wire codecs live in
`oer-protocols-ieee80211-wire`, policy in the cores above the port, vendor records in
`oer-chips-family-radio-ieee80211-policy` and chip models in the chip's MAC; no normal
dependency of the port reaches an ieee80211 core (`oer-tools-test-layers`). It is
sans-IO: it declares the port and its values and never waits on it.

## The data port

The station's datapath actor owns the radio as a `DataPort` (module
`data_port`) by value for an installation's whole life and calls it in its
turns: it waits on the port's signals (`poll_facts`) and one alarm at
`deadline`, reads the clock once per turn and passes that `now` to every
call. No port method wakes its caller, reads the clock or loops over a
queue. The station's lifecycle never holds the port: it sends each key,
receive-configuration, gate or setting operation as one `PortCommand`
(module `command`), which the actor hands to `DataPort::command` in order.
A command whose barrier is not reached comes back `Held` and runs again on a
later call; one the port cannot run in its state comes back refused with
every owner. Installation commands come only from the backend's issuer
(`PortCommands`), which mints the identity the installed key will carry.
Terminals, TBTTs and the units a group mutation's receive cut drained go to
the turn's `PortSink`, in the order the port produced them; a data arena's
end returns from `service_tx` itself. `PortTypes` (module `types`) names
the types the port's owners and terminals carry:

- **Tickets and buffers.** A transmission takes a ticket (a place in
  flight and its terminal's place) and the buffer of its frame, of a
  class sized by the composition (`TxBufferClass`, `ClassFrame`); its
  `TxCompleted` terminal gives both back with `PortTypes::Context`,
  chosen by the composition with `type Context: 'id` in `PortTypes<'id>`.
  `PortTypes` also owns the single body type and backend fault type.
  Before submission, `Attempt::admit` seals context, ticket and the
  prepared MPDU into one closed `InFlight`; `DataPort::admit` accepts the
  sealed `StationSubmission` and `DataPort::submit` queues it.
  Queues, publication, withdrawal and quarantine retain that whole owner.
  `TxCompleted::Single` returns its `Completed`. `TransmitPool` mints
  tickets and one `TerminalAccess` for the consumer.
  Only consuming `Completed::into_parts` with that installation- and
  pool-specific permission releases owners. Completion alone grants no
  unpacking authority, and raw `InFlight` admission is private.
  Backend execution uses `ExecutionPayload`'s narrow projection; it may lend
  storage needed by the driver but may not expose a replaceable MPDU or
  owner extraction. The model projections keep those fields private, and
  host/RV32 UI pairs reject swapping tickets, contexts or MPDUs and
  reconstructing a terminal through fields.
  Context moves unchanged; no `Clone` bound, frame inspection or parallel
  lookup identifies the original operation. Bodies remain owned until the
  same terminal, even when the backend copies their octets into DMA memory.
  An empty pool is the caller's own capacity; an occupied queue is the
  backend's, which keeps the attempt until the queue frees.
- **Preparation and submission.** `TxBufferPool::prepare` checks a source
  where its buffer class is concrete and returns a closed `Prepared` value.
  `TxSource::check` first consumes the raw source into `CheckedTxSource`;
  layout errors return the source before entering the backend. Only a
  `ResponseHeader` (`AckHeader`, `NoAckHeader` or a group-addressed
  `HeaderSpec`) encodes; its type selects the QoS Ack Policy, with no
  separate flag at encoding. The octets are the wire `MacHeader` codec's,
  which refuses short storage before writing. Storage
  shortage, CCMP shortage and body overlap are `LayoutError`; backend
  `PreparationError` contains retransmission identity/kind refusals.
  It retains the port borrow, install, role and key borrow. `ClassSizedFrame`
  carries a portable machine's size proof until the service adds port-bound
  rates, without rechecking the class. `TxProtection::{Open, Ccmp(NumberedKey { key, packet_number })}` in
  the fresh source is the sole source of the Protected bit. It derives
  `MpduProtection` for the layout; callers cannot change a header's protection
  independently through a public setter. Station TX keys
  are pairwise; AP TX may use pairwise or group keys. The portable
  `MpduLayout` constructor checks CCMP room and tail-body overlap once,
  before any header/body write or admission. The core reserves a PN before
  handoff; cancelled preparation may leave a gap, and a reserved PN is never
  allocated to another MPDU.
  `PortRate` retains the capabilities and the borrow of the port that
  checked it (`DataPort::rate`, `PortArena::rate`) through NoAck
  (`PortRate::no_ack`), legacy (`PortRate::legacy`) and A-MPDU
  (`PortArena::ampdu_rate`) rates, retry plans and prepared frames. A start
  retunes only under `&mut`, so a rate checked in one turn is not used after
  the port moved on (`tests/ui/port_rate_turn.rs`), and a rate of other
  capabilities does not fit (`tests/ui/rate_capabilities.rs`). These
  proofs add no stored bytes.
  Preparation borrows `TxBody::bytes` once and keeps that view for both the
  length check and the complete tail copy. Re-reading an external body can
  select another slice; even a shorter second view must not leave stale
  payload bytes in an accepted MPDU. The body owner remains with its source.
  A checked header exposes `header_len`, and its MPDU layout exposes
  `mpdu_len`; neither has an empty state. Block Ack request lengths come from
  the MAC wire encoder's `BLOCK_ACK_REQUEST_LEN`.
  A refused preparation returns the original typed source and its owners;
  the caller still holds context and ticket. `submit` takes the already sealed
  operation and its attempt parameters; a refusal (the radio does not run
  or stops) ends it at once, withdrawn, with its terminal in the sink.
  The queued operation owns backend data, never a borrow of a local machine.
  A retransmission takes the returned `Sent` MPDU and a port-bound
  `RetransmissionPlan`: header, payload, body and installed-key identity
  remain inside that MPDU. SN and PN stay; only Retry changes in its bytes.
  Rate, retry plan, RTS/CTS, power and the coexistence operation belong to the new attempt.
  Retiring the original installation makes preparation refuse
  `RetransmissionKey`, even if the slot and key material are reused.
  `renew` returns memory and body explicitly.
- **Data arenas.** The station's data goes through two DMA arenas the
  port lends for one turn (`DataPort::arena`, a `PortArena` whose memory is
  `PortTypes::DataMemory`, the network's own frame memory, and whose
  evidence and fault are the port's). The caller writes each frame once as
  an arena takes it and publishes one arena at a time as an A-MPDU at an
  `AmpduRate` or as the single MPDU of its position 0 under a `RetryPlan`,
  which the backend carries out as for any single MPDU; the backend serves
  the hardware and returns the end from `DataPort::service_tx`. A backend
  publishes nothing while its gate is closed, a configuration is pending or
  it stops: it refuses the publication as busy.
- **Management plans.** `ManagementTxPlan` carries a legacy rate derived
  above the driver and a nonzero attempt budget; every attempt keeps the
  selected rate. `oer-protocols-ieee80211-core-upper-mac` `management` chooses the budget:
  seven attempts (dot11ShortRetryLimit) for a unicast frame, one for a
  broadcast.
- **Retry plans.** An individual frame carries its `RetryPlan`, a sequence
  of rates with their transmissions and protection (including each control
  frame's legacy rate), as mac80211's rate
  table. The first stage is mandatory; the tail is a fixed array of
  `RETRY_STAGES - 1` cells whose occupied cells form a prefix, which every
  operation keeps, so mapping and filtering a plan are total and cannot
  exceed the bound. Filtering a plan keeps
  each accepted stage's full budget and order, within the original bound.
  The backend retries the head of its queue by the plan before the
  next attempt of that queue; the completion reports the transmissions made
  and the last rate, for the rate control above the port.
  A single MPDU ends as `SingleOutcome` (`TxOutcome<SingleEnd>`); a data
  arena's A-MPDU teaches rate selection as `AggregateOutcome`
  (`TxOutcome<AggregateEnd>`), so neither can carry the other's response. `Air` holds the exact nonzero MPDU count
  together with the last transmitted rate (`u16` covers all four
  255-transmission stages); a delivery and an ACK timeout always carry it,
  while a protection failure, contention exhaustion and lost air carry
  `Option<Air>`, `None` when no MPDU reached air. A collision at a later rate
  preserves the preceding air transmission's count and rate. A delivered
  single MPDU names its response: unsolicited, an ACK, or a BlockAckReq's
  BlockAck with its report. A delivered aggregate always carries its
  BlockAck report. The backend's evidence of the response is its own type:
  `PortTypes::AckEvidence` for a single MPDU, which a policy needing the
  ACK's SNR requires as `AckSnr`, and `PortTypes::BlockAckEvidence` for an
  aggregate, split by `BlockAckSnr` into a measured sample or none, because
  hardware may accept a BlockAck without a measurement.
  The outcome carries no RSSI. `AirDenied` carries `PortTypes::AirDenied`,
  `Infallible` for a backend that cannot tell a coexistence abort apart.
  A hardware timeout with no completion ends as `Indeterminate`;
  the backend does not republish that MPDU, stop the queue, or return its
  tail. `TxStatus` describes only Success, AckTimeout, CtsTimeout, Collision
  and TimedOut observations of a publication. Decoder/accounting faults
  are the port's named fault and produce no `TxOutcome` or returned-resource
  event.
  Internal collisions remain driver retries; budget exhaustion is
  `ContentionLimit`. Withdrawals name only GateClosed, KeyRemoved or
  Disabled. Rate selection ignores withdrawals, Indeterminate,
  ContentionLimit and AirDenied (`SingleEnd::rate_sample`). A barrier waits for one current publication,
  bounded by the backend's publication bound, not the remaining plan.
  ACK still ends as `Delivered`; a needed retry returns
  `Withdrawn(cause)` with Sent, retaining SN/PN. An unpublished attempt
  returns Fresh. Unknown timeout remains Indeterminate. Per-AC terminals
  precede the barrier confirmation in their original order.
- **MSDU age.** Each VIF's EDCA table stores a typed `oer_portable_time::Duration`
  per AC (default 500 TU); `EdcaParameterSet::from_tu` converts MIB units
  at the record boundary. Admission samples backend `now()` at `submit`,
  once per MPDU. Every publication and retry checks age. Hardware-owned
  attempts finish independently of age. `Expired { msdus }` returns all
  original owners, bit zero naming the MPDU. A data arena's frames age by
  the caller's admission instants instead. Renewal for a new MSDU clears the old sample through
  `TxBufferMemory::renew_msdu`. Both buffer accessors return
  `BufferMemoryUnavailable` when the memory is not the body's (its hardware
  owns it, or its place holds none); `ReturnedMpdu::into_fresh` always
  returns the token with that result. Expiry is counted separately and supplies no
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
  and `PortTypes::BlockAckPool` must be that pool (`RxBlockAckPool::LEN`).
  A declared capacity other than the minted one does not compile, the
  capabilities carry no copy of these counts, and running out is a missing
  slot token, never a comparison. A pairwise slot has one installation or
  removal command and terminal outstanding, since removal needs the key only
  the installation terminal returns. The command bound is tickets + the
  pairwise slots + one GTK permission per interface + configure permissions
  + two gate commands. The terminal bound is tickets + pairwise slots + GTK
  permissions + gate closing + configure permissions; received units are
  bounded separately.
  `PortTypes` exposes these bounds. `InterfaceCommandPool` derives both of
  its contributions from the same constants used to mint its slots.
- **Interface commands.** `Mint` gives each interface one affine
  `ConfigurePermission`, branded by installation, role and pool path.
  `PortCommand::Configure { permission, receive }` enters the command order;
  `Configured` returns the same permission with its opaque context.
  The station's connection epoch travels in that context, so the service
  does not match a role against a separate pending-request table. A second
  configure is impossible until the first terminal returns. Configuration
  withdraws held attempts of its interface and waits for its published
  attempts; another interface's publication does not block it.
  A stop drains these command terminals with their owners.
- **Settings.** EDCA (`PortCommand::SetEdca`), BSS color
  (`PortCommand::SetBssColor`), the HE association (`PortCommand::SetHe`)
  and Block Ack desired state take effect before the next publication; the
  latest value of each wins. `SetHe` carries `StationHe` in the pinned vendor
  station's two stages: `Bss` (HE BSSID matching, the color bitmap cleared,
  the BSS color the scan heard) before authentication, `He20` (the access
  point's HE parameters from its Association Response, the AID and the
  minimum MPDU start spacing, each within its field) after the Association
  Response and before the link's data. `Off` starts a join of a BSS without
  HE and ends an HE association. `Bss` and `Off` also set the color of the
  station's HE transmissions. Each VIF keeps its
  complete EDCA table: AIFSN, ECWmin, ECWmax, TXOP limit and MSDU lifetime.
  `SupportedEdca` refuses unsupported records; the port applies them
  exactly. The head frame's VIF selects the parameters of its AC queue's
  single contention state. Before a BSS is known, the standard non-AP
  defaults apply (IEEE 802.11-2020 9.4.2.28, Table 9-155), with 500 TU
  lifetimes. Local lifetimes are separate from the on-air WMM element.
  TXOP limits are retained in 32-us units; an access sends one frame.
- **Gate evidence.** The install includes one `GatePool` in its declared pool set.
  Its private `GatePlace` starts inside `GateOpen`
  and retains one `GateIssuer` in the backend. Closing moves that place to
  the driver; only after the publication barrier does the issuer transform
  it into `GateClosed` for its terminal. Opening (`DataPort::open_gate`)
  consumes that evidence and returns the same place as `GateOpen`. There
  are no independent open and closed slots. The arbiter holds these exact
  branded states; the caller can neither construct closed evidence, copy
  the place, nor keep both states.
- **Reception.** `DataPort::receive` gives one received unit at a time,
  while the caller holds fewer than its room; the unit goes back through
  `recycle` or moves to the network through `lend`.

`oer_protocols_ieee80211_contracts_security::Ccmp128Key` is the shared affine, zeroizing owner
of installation material. RSN moves this owner into installation. Its
bytes are private; hardware programming and software CCMP borrow them.
It has no `Clone`, `Copy`, equality or owned byte extraction, and Debug is
redacted. The model moves that same owner through its installation command
into the key table, where removal or reinstallation zeroizes it
independently of any surviving slot identity. Quarantine intentionally retains the key without
calling `Drop`, until chip reset.

An `AssociationPool` mints one start permission and the complete PTK/GTK
bank of its association domain. Beginning consumes that permission and fixes
`Spp::{Negotiated, NotNegotiated}`. Every key installation borrows this owner
and consumes a slot from the same installation, role and pool path. Keys
cannot choose independent SPP modes. The backend applies the association's
mode before publishing its first key; its `SppScope` capability declares
whether domains occupy whole interfaces or individual associations.

A pairwise installation consumes its slot and reserves a FIFO terminal;
only `PairwiseKeyInstalled`, after the write took effect, returns the
installed key, so no attempt names it earlier. A failed write is the
port's named fault, with the slot and association retained. Pairwise removal carries the
association owner through its `KeyRemoved` terminal. The station first removes its group keys, then the PTK; spare
unused slots remain owned throughout. Only the exact complete `VacantKeyBank`
can enter `end_association(owner, bank)`, which synchronously returns a
`VacantAssociation` for the next exchange. No command or terminal is added.
A partial bank or quarantined key cannot release the association permission.
On an AP, carrying the owner through a pairwise removal temporarily defers
new key installations until that terminal returns; existing keys retain
their immutable mode and admitted transmissions proceed normally.

Each interface receives one affine `KeyTypes::GroupPermission`, minted once
with the installation and branded to that port and interface. Every GTK
install, replacement, selection and removal consumes it and reserves a
terminal. The command transfers resources; `GroupKeyInstalled`,
`GroupKeyReplaced` or `GroupKeySelected` alone proves success and returns the
installed owner and permission. `KeyRemoved` returns the vacant slot and
permission. There is no Busy result or timer retry for admission.

Exactly one installation per association/wire KeyID decrypts at a time;
other IDs may stay active. Each successful GTK terminal is a real RX cut:
all delivered old-installation frames precede it, including facts retained
in hardware, DMA, ISR and software queues; selected-installation frames
follow it; the units the cut drained go to the sink before the terminal.
The cut is finite under continuous ingress. A mutation held at its barrier
past the backend's command bound is the port's named fault, which keeps its
owners. A stop completes accepted GTK operations within the same bound. The
API exposes no cancellation.

`PortCommand::SelectGroup` consumes a concrete retained installed owner. It neither
writes material nor creates an installation nor resets counters, including
when already current. A higher layer retains each saved key's replay owner
and exchanges it at the terminal. The port knows no replay state.
`KeyTypes::GroupSlotIdentity` correlates pending actions with their terminal;
`Identity` still names the actual material installation used for TX.

The model and the S31 port check retained same-ID selection and delayed
ingress against the shared conformance verifier. An evicted key is no longer installed and
cannot be selected from a RAM copy. Physical slot counts belong to backend
pools; each hardware backend must prove its own RX boundary.

The install mints one `Slot<ConnectionEpochKind>` for Station's epoch source.
This slot kind belongs to the lower-MAC contract; the backend neither depends
on the station's epoch implementation nor creates connection identities.

A frame's kind (`TxFrame`) fixes its response: an individual frame (an
`AckHeader` with an `IndividualAddress` receiver, any data or management
frame but Action No Ack), an individual frame without an ACK (a
`NoAckHeader` at a `SupportedNoAck` rate), a group frame or a BlockAckReq
(at `LegacyPortRate`s). Each role has its own receive rules
(`StationReceive`, `SupportedFilter` of the role).

The module `owner`'s table maps every refusal a caller's own state explains
to its type, and lists the world's outcomes that stay. Compile-fail tests in
`owner`, `types`, `command`, `data_port` and `tests/ui` pin each rule a type
carries.

## The access-point role

The data port is the installation's one physical owner: the MAC, the
receive ring, the interrupt owner, DMA and the transmit queues, the
station's data arenas, the gate and the start and stop. The access point
reaches it through `AccessPointPort` (module `access_point`) beside the
station's entry points, with no runtime role switch: `admit_access_point`
seals its attempts (a pairwise or group `AttemptKey`), `access_point_command`
runs its `AccessPointCommand`s and its issuer (`AccessPointCommands`) mints
its key installations. Both roles' attempts go to the same `submit`, the
same queues and the same sink; a backend runs one implementation for both.
Received units come from the one ring for both roles.

The access point's permissions are its own: its configure permission, its
association, its pairwise slots (one per association identifier), its group
slots and group permission, and the installation's one `BeaconPermission`
(minted by `BeaconPool`). A missing permission does not compile, and no
station permission or installation payload fits an access-point operation or
the reverse (`tests/ui`, the model's `tests/ui`). A `BeaconAttempt` takes the
permission, contends as `BEACON_ACCESS_CATEGORY` (voice) unprotected, and
every end of it, a refused submission's included, is a `TxCompleted::Beacon`
that returns the permission: one beacon is admitted, queued or on the air at
a time.

Every token the port's own pools mint (configure permissions, association
starts and key slots, the beacon permission, and a backend's group
permission when it uses `IssuedSlots`) is an `IssuedSlot` that names the
generation of the installation that issued it. A pool yields a grant
(`SlotGrant`, `InterfaceGrant`, `AssociationGrant`, `BeaconGrant`); only the
backend that consumed the mint holds it and issues it once with its
installation's generation. After a reinstallation of the same identity an
earlier token still compiles; the port refuses it at the acceptance point
with its owners: `submit` refuses a beacon or an access-point attempt under
an earlier permission or key (`SubmitRefusal::StalePermission`, terminal
`Withdrawn(StalePermission)`), and `access_point_command` refuses a command
naming an earlier permission, association, slot or key
(`AccessPointRefusal::StalePermission`, the command back). The station's
operations refuse the same way: `command` with
`CommandRefusal::StalePermission`, `submit` a frame under an earlier key.
Tokens without a generation (the gate, transmit tickets and buffers, receive
Block Ack slots, the epoch slot) are not checked.

| Operation | Accepted at | Refused before | Ends with |
| --- | --- | --- | --- |
| access-point transmission | `submit` | `SubmitRefusal`, withdrawn terminal in the sink | `TxCompleted::Single` |
| beacon | `submit` | as a transmission, an earlier installation's permission included; its terminal returns the permission | `TxCompleted::Beacon` with the permission |
| key install or removal, configuration with its TSF control | `AccessPointProgress::Done` | `AccessPointProgress::Refused` with the command (`NotEnabled`, `StalePermission`) | its terminal (`PairwiseKeyInstalled`, `GroupKeyInstalled`, `KeyRemoved`, `Configured` of the access point) |

The access point's commands run in its own order: one held at its barrier
holds no station command, and a station command held at its barrier holds
no access-point command. A configuration withdraws the queued attempts of
its own interface and waits for its published ones only. The access point
stops with its own commands (configuration `Off`, the removal of each
pairwise key and of the group key, a configuration stopping the TSF), which return its owners
and leave the station's attempts, keys and configuration as they were. A
confirmed `poll_stop` stops both roles: every queued attempt ends withdrawn,
the access point's interface receives nothing and runs no TSF, and each
role's permissions stay with their holders.

Who writes a beacon's Timestamp field (`BeaconTimestamp`) and whether the
TSF restarts and stops by a sourced sequence (a configuration carries the
control, with the configure permission) are the backend's
`AccessPointServices` (`AccessPointCapabilities`). An unsourced value is
absent, and then no `BeaconStamp` or `SupportedTsfControl` exists for that
backend: the operation is refused before it is built.

## Data on two arenas

`TxArena` (module `arena`) is the data transmit path: the backend owns two
arenas of DMA slots (`ArenaId`), the caller one affine `Arena` right per
arena. The caller fills an arena frame by frame (`intake`, which writes the
MPDU once into the memory the caller hands over and names its `FrameIndex`)
and publishes it as one A-MPDU of every frame or one single MPDU of the frame
at position 0, the others staying unsent in the arena (`ArenaPublication`: format
and port-checked rate, access category, RTS/CTS protection, encryption
under the installed pairwise key, power). The backend refuses a frame to
send whose Protected bit the encryption contradicts and an A-MPDU past its
hardware octet or subframe limit. Publication moves the right into
the backend; one arena is on the air at a time. `service` reports the end
(`AttemptEnd`) as an affine `ArenaCompleted`; only `retain` (keep a
`FrameMask` of frames, compacted in order, the Retry bit set when a frame
may have reached the air, on the frames the publication sent) or `release`
consume it and return the right, so
a completed arena is settled before it is filled or published again. Every
returned memory comes back in position order. Sequence and packet numbers,
BlockAck windows, ageing and the retry policy stay with the caller.
Required methods are the backend's; the provided ones alone mint rights,
indices and completions (`tests/ui/arena_*.rs`).

## Optional features

The rates a backend aggregates at are its arenas'
(`PortArena::ampdu_capabilities` with its per-rate `AmpduLengthTable`,
checked into an `AmpduRate` by `PortArena::ampdu_rate`), which a data
arena's A-MPDU takes. A TBTT of the station's schedule comes in the sink
(`PortSink::tbtt`, `TbttEvent`). The values of these features (`TsfSample`,
`VifTsf`, `MonitorCapabilities`) are in `extensions`. The backend that
writes an interface's TSF keeps that TSF's relation to the radio clock and
stamps each sample with its generation. The TSF clock domain
(`tsf::TsfInstant`, `TsfDuration`, `time_units`) is the port's: a received
beacon's Timestamp field (the wire's `TsfTimestamp`) becomes a `TsfInstant`
through `tsf::timestamp_instant`.

Parametric limits of what a backend has are its capabilities
(`LowerMacCapabilities`), and a value outside them has no checked type. Why
a backend lacks a feature or a value (hardware absent, glue not written,
vendor knowledge not recovered, policy decision pending) is recorded in its
documentation, not in code.

The PHY and channel values come from
[`oer-protocols-ieee80211-wire`](../../wire/src/phy.rs): `Channel` (2.4 GHz and 5 GHz with
20 MHz and 40 MHz geometry) and `PhyRate` (non-HT, HT and HE SU).
`RxEvidence` and `RxCryptoStatus` describe receive provenance. The values a
backend and the layers above it share live here (`contract`): the service a
backend describes (`MacServiceCapabilities`, `MacOperationOwnership`,
`MacResourceLimits`, `MacInterfaceCapabilities`), its receive metadata
(`MacRxMetadata`) and the terminal status of one logical MPDU exchange
(`MacTxStatus`, `MacTxResult`), which both the S31 direct ordinary path and
the upper MAC's planner report. `MacOperationOwnership::hardware_services`
turns a service description into `HardwareServices`. A VIF's EDCA table is
an `EdcaParameterSet`; the backend keeps each queue's contention window.
The monitor boundary (`monitor`) carries the tap point, the channel
context, the observed frame, the sink and the injection request; the
injected MPDU's geometry is the wire's `monitor_injection`.

## Above the port

What a backend does not report in its `HardwareServices` is software above
the port, written once in portable packages for every backend, with one
exception: the backend draws contention for its hardware queues. The core
allocates protected MPDU packet numbers and the backend formats the supplied
PN with the installed key ID.

| Work | Owner |
| --- | --- |
| The retry plan of an MPDU and the rate control that chooses it | `oer-protocols-ieee80211-core-upper-mac` |
| What a `BlockAckReport` acknowledges | `oer-protocols-ieee80211-core-upper-mac` `block_ack::acknowledges` |
| Which frames of a data arena go again after a `BlockAckReport`, the BlockAckReq, aging | the station's data plane and transmit agreements (`oer-protocols-ieee80211-core-roles-sta`); `oer-protocols-ieee80211-core-upper-mac` `ampdu` holds the vendor's per-aggregate machine |
| RTS/CTS or CTS-to-self, the control-frame rate and the BSS basic rates | `oer-protocols-ieee80211-core-upper-mac` `protection` |
| The retry counter a frame's class advances and the ladder step of its failures | `oer-protocols-ieee80211-core-upper-mac` `retry` |
| The attempt budget of a management frame | `oer-protocols-ieee80211-core-upper-mac` `management` |
| The PHY mode of an association | `oer-protocols-ieee80211-core-upper-mac` `phy_mode`, chosen by `oer-protocols-ieee80211-core-roles-sta` `association` |
| Transmit packet-number allocation and receive replay | `oer-protocols-ieee80211-core-upper-mac` `ccmp` |
| Receive Block Ack agreements and reordering | `oer-protocols-ieee80211-core-upper-mac` `block_ack::banks`, `block_ack::recipient`, `block_ack::reorder` |
| The station's lifecycle, from its scan to its disconnect | `oer-protocols-ieee80211-core-roles-sta` `machine` |
| The Wi-Fi owner topology checked against a backend's service | `oer-protocols-ieee80211-core-extensions-esp-now` `configuration` |
| Which frames a monitor keeps and the channels it visits | `oer-protocols-ieee80211-core-upper-mac` `monitor` |
| Virtual-interface identities and ESP-NOW PHY requests | `oer-protocols-ieee80211-core-extensions-esp-now` `interface`, `extensions::espressif::phy` |

Limits, rate ladders and duration estimates in which implementations differ
are parameters of those algorithms; the Espressif values recovered from the
vendor stack, including the opaque rate-schedule records and the default
contention of each access category, are the family package
`oer-chips-family-radio-ieee80211-policy`.

## Implementers

The host model in `oer-protocols-ieee80211-testing-model` implements the data
port and the access-point role (`ModelPort`) with composition-selected
context types. A stop returns
queued operations, including queues held by the model air, and a quarantine
keeps every owner the port holds. Hardware backends provide the same
resource and terminal contract.

The S31 driver implements the data port, not yet the access-point role: it
mints the access point's permissions and its station-only split drops them.
It implements this contract in its
[`port` module](../../../../chips/esp32s31/radio/ieee80211/backend/src/port.rs)
(`Esp32s31Port`, statically dispatched). It drives `LowerMacCore`,
cooperative registers and the RX ring; P3 posts only static typed signals.
The image supplies the timer, entropy and opaque contexts. Ordinary MPDUs
retain caller-supplied PNs and external bodies; a reinstallation starts from
the stopped resources.

Station group-key slots are fungible, affine resources. The backend stores
and returns each raw slot with its installed key; it carries no station-cell
wrapper. Station returns removed slots to its declared free pool. Its full
pool proves that no group install or removal remains outstanding. Pending
installation tokens carry the protocol key ID and connection epoch.


The backend TX forms are caller-selected single attempts and fixed-rate
`ManagementTxPlan`s. A backend neither walks a vendor retry ladder nor
selects BSS protection or a protection control-frame rate. The S31 queue
adapter receives the exact control-frame rate through each `RetryPlan`
stage's `Protection` value. EDCA backoff and MSDU age remain backend work.

`backend` carries the values a backend shares with the family policy and
the retry owner: a rate-schedule record's format family (`RateScheduleKind`)
and a frame's retry counters (`RetryCounters`). A chip's
contention, transmit-cost model and publication work are its own, as the
S31 MAC's `edca::contention`, `tx_cost` and `tx_work`; its direct ordinary
path takes a `MacTxPlan` from the S31 driver's `ordinary_tx`.

## Shared backend conformance

The backend conformance suite is
[`oer-protocols-ieee80211-testing-conformance`](../../testing/conformance/README.md);
the port carries no test code of its own.
