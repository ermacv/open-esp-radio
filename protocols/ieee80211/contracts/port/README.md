# IEEE 802.11 port contract

`oer-protocols-ieee80211-contracts-port` declares the radio port between
portable Wi-Fi MAC logic and the backend that puts frames on the air, as
[the architecture](../../../../docs/architecture.md#radio-ports) describes
it. It holds only the interaction with the peripheral and the
representation of the data that crosses it: rights, outcomes and facts.
Wire codecs live in `oer-protocols-ieee80211-wire`, policy in the cores
above the port, vendor records in `oer-chips-family-radio-ieee80211-policy`
and chip models in the chip's MAC; no normal dependency of the port reaches
an ieee80211 core (`oer-tools-test-layers`). It is sans-IO: it declares the
port and its values and never waits on it. No call of the port returns an
error.

The port is the owner of its hardware in the sense of RAII: what an
operation sets up, its opposite takes down, and a stop returns the port to
its state before the start. The hardware's anomalies reach the cores as one
indication with no payload, "the port needs a stop"
(`Facts::needs_stop`), which the installation machine
(`oer-protocols-ieee80211-core-installation`) answers with the stop it has.

## Rights

Every right of an installation is a value branded by the installation
(`'id`) and made only by the contract, from the port's `PortKey`; a key
comes only from an installation's mint (`PortPools`, through
`oer-protocols-radio-port`'s `Mint`), with the radio stopped, the stop's
right, the rights of the installation's layout (the radio's and each
interface's, [Interfaces](#interfaces)) and the installation's closed gate
(`Install`). A transition consumes the right it takes and returns the next,
so a call with a spent, a stale or a foreign right cannot be written
(`tests/ui/forged_right.rs`, E0451). The one door around the key is
`life::forge`, which exists only with the `test-support` feature; no normal
or build dependency turns that on (`oer-tools-test-layers`,
`a_right_is_forged_only_by_tests`).

| Right | Made by | Held by | Taken by |
| --- | --- | --- | --- |
| `Stopped` | the install, a confirmed stop, a failed start or retune | the installation machine | `start`, `retire` |
| `Starting`, `Retuning`, `Stopping` | `start`, `retune`, `stop` | the machine | their polls |
| `Running` | a confirmed start or retune | the machine | `retune`, `stop` |
| `StopRight` | the install | the machine, lent to a stop | `release` |
| `Retired` | `retire`, with `Stopped`, every bundle of the layout and `UnitsHome` | the machine | `release` |
| `GateClosed` | the install; the close-gate command's end | the radio's controller | `open_gate`; lent to a retune (`GateLent`) |
| `GateOpen` | `open_gate` | the controller, lent to a publication | the close-gate command |
| `IdleArena`, `Arena`, `OnAir` (the radio's) | the install; `open`, `publish`, `settle`, `withdraw` | the engine | the next of them |
| `QueueRight` (the radio's) | the install; a single's end (`PortSink::single_ended`) | the engine; the port from a single's publication to its end | `publish_single` |
| `SinglePlace<'id, I>` (an interface's) | the install; a single's end, as `I`'s | the engine; the port from a single's publication to its end | `publish_single` |
| `CommandPlace`, `InCommand<'id, H>` (the radio's) | the install; a kind's `begin_*`; its end | the engine | a kind's `begin_*`, `poll_*` |
| `PairwiseSlot<'id, I>`, `GroupSlot<'id, I>`, `AgreementSlot<'id, I>` and their installed forms (an interface's) | the install (empty); a key's or an agreement's command end | the engine | the commands that install, replace and remove them |

## Interfaces

An installation runs a closed set of interfaces of the product:

| | Address | Receive rule | Keys | TSF | Agreements |
| --- | --- | --- | --- | --- | --- |
| station | its own | its BSS | one pairwise, its group keys | the access point's, followed | its access point's |
| access point | its own, its BSSID | its BSS | one pairwise a client, its group keys | its own | its clients' |

What differs between two interfaces of different roles is counts: pairwise
key places, group key places, agreements' places, single places, data
places. So the rights are two kinds of bundle:

| The radio's, once (`RadioRights`) | An interface's (`InterfaceRights<'id, I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>`) |
| --- | --- |
| the two arenas | its single places, by purpose |
| a right a transmit queue | its role's data places (`I::Data`: none for a station, one a queue for an access point) |
| the command's place | `PAIRWISE` pairwise key places: one for a station, one a client for an access point |
| | its group key slots |
| | its receive agreements' slots |
| | its receive rule's right, off (`ReceiveOff`) |

The life, the channel, the gate and the receive ring are the radio's and
have no place in a bundle.

**An interface is a type.** Every right of an interface carries its role
`I: InterfaceRole` (sealed: `Station`, `AccessPoint`), its index the
interface's own; a command and its held and end values carry it too. A
port reads the hardware's selector from the type: a publication names its
interface by its place's type and takes no other argument, and an arena's
interface is its link's (`DataPort::Link<I>`, `open::<I>`). A right of one
interface does not build in another's call
(`tests/ui/foreign_interface_single.rs`, `tests/ui/foreign_interface_key.rs`).
An installation has at most one interface of a role.

**The layout** is a tuple of one or two interface layouts
(`InterfaceLayout<I, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>`): its mint
gives `(RadioRights, the bundle)` for one interface and
`(RadioRights, (the bundles))` for two, and its one check, evaluated
where it is named, refuses two members of one role
(`tests/ui/one_interface_a_role.rs`). A port serves the layouts its
`RadioPort` is implemented for (`RadioPort::Layout`); a layout it does not
serve does not build. A single the port published ends through the sink
as its interface's (`Layout::Single`: the place itself for one interface,
`SingleOfTwo` for two): the port keeps what it published as a
`PublishedSingle`, one a single on the air, and its key's end takes it and
makes the place again.

**The receive rule is a right.** An interface's bundle holds
`ReceiveOff<'id, I>`. `ReceiveCommand::Set` gives the port the interface's
own address and its role's rule (`I::Receive<K>`: `StationReceive`,
`AccessPointReceive`) and ends `Receiving<'id, I>`; `Change` takes and gives
back `Receiving`; `End` takes `Receiving` and ends `ReceiveOff`. As a key's
command, its end always returns the right, set or off: a setting that
expires ends off, the hardware as before it; a change that expires ends off,
the port having cleared the address and the rule. Receiving nothing is the
right off, no value of a rule.

**Who holds what.** The machine holds every bundle while the data is
stopped (the radio stopped, or running with the data stopped); the engine
holds them while the data flows. An engine has one interface: its form
takes the radio's bundle and its interface's at the data's start and gives
both back at its stop. An interface's bundle, whole, is the proof that
the interface is taken down in the hardware: its singles home, its keys
removed, its agreements ended, its address and receive rule cleared.
`retire` takes every bundle.

## The radio's life

`RadioPort` (module `life`): `start(stopped, channel, now)` and
`poll_start`, `retune(running, channel, gate_lent, now)` and
`poll_retune`, `stop(&mut StopRight, from)` and `poll_stop`,
`poll_units_home`, `retire(stopped, data, units)` and
`release(retired, right)`. A start or a retune that fails answers the radio
stopped with its `StartFailure`; a stop always ends in `Stopped`. A retune
is begun under the installation's closed gate, which the request lends to
the step where the port's retune begins. There is no restart and no
reinstall: a port is installed once.

## The data path

`DataPort` (module `data`): `poll_facts`, `deadline` and `sample_clock`;
the rates the port sends and aggregates at, checked once and branded by the
installation (`rate` gives a `PortRate`, `ampdu_rate` an `AmpduRate`).

**An aggregate is a place, filled and published once.** `open` opens an
idle arena for the link (which carries its key) and a TID; `place` gives
the next frame's `Place`, or `Complete` when the arena's slots are full,
the one answer of the path that is not an outcome. `Place::write` takes the
MSDU's memory by its handle, in place, and lends the headroom in front of
the body (`HEADROOM`: a QoS data header of three addresses, the CCMP header
and LLC/SNAP, 42 octets, rounded up to the body's word alignment, 44) for
the engine to write the MPDU's front flush against the body; `HeaderLen`
says where the MPDU begins. The headroom holds what the network wrote
there: a data MSDU's Ethernet header is its last 14 octets.

**A transmit slot is the port's memory around a body.** A port states two
numbers of its transmit memory: `DataPort::TX_FRONT`, the octets it needs
in front of the headroom (what its hardware reads before the MPDU), and
`DataPort::TX_BEHIND`, the octets behind a body and its MIC's room
whatever the cipher (what its hardware appends: an FCS). The MIC's room is
the contract's one constant, `MIC_ROOM`: the longest MIC of the ciphers it
carries (CCMP's 8). Nothing above the port writes the port's octets.
`tx_slot::<P>(body)` is a slot's length for a body of `body` octets; the
body's length is not the contract's number but the slots' owner's. The
owner names its slot in a const item, `TxSlotOf::<P, BODY, LEN>::SLOT`, built
by `TxSlot::new`, which refuses (`TxSlotRefused`) a body that would not lie
on a word (the network writes an IP datagram there and reads its header in
words), a slot shorter than `tx_slot` and a body whose longest MPDU (the
engine's longest front, 42, and the body) is longer than the capability's
`max_mpdu_length`, which counts the MPDU without its MIC and FCS (IEEE Std
802.11-2020, 9.2.3, 12.5.3.2); the headroom holds each role's longest front
by the role's own const check. A geometry it refuses does not build.

`publish(arena, how, gate, now)` puts the arena on the air as its `Ppdu`
says: `Single(plan)` its first frame as one MPDU, acknowledged, by the
plan; `Aggregate { rate, spacing, protection, alone }` the longest prefix
of its frames the rate admits (the per-rate length, the TXOP's airtime) as
an A-MPDU at the recipient's spacing behind the protection, and a prefix of
one frame alone by `alone`. Nothing is refused; the frames that did not go
stay in their places with no attempt spent. `aggregate_ended` reads the end
in place (`Ended`: an `ArenaOutcome` that says what went, `Single` or
`Aggregate`, the frames sent and those acknowledged); `settle` keeps the
frames the engine names for a retry and sends every other body home
(`Settled::Kept` or `Settled::Idle`); `withdraw` sends an arena's bodies
home. The transmit section's order is the end, the standby published at
once, then the settlement, then `serve`.

**A single MPDU** (management, EAPOL, Null, a beacon) is written into its
purpose's place (`write_single`) and published on a free queue
(`publish_single`, with its `TxFrame`). From its publication to its end the
place and the queue's right are the port's; its end gives them back through
the sink (`PortSink::single_ended`). A role's frames are of a closed set of
purposes and the installation has a place for each (`SINGLES`), so a
role's frame always has its place.

**Commands**, one kind a call, each carrying the right it needs:
`CloseGate` (`begin_close`, `poll_close`), `PairwiseCommand`
(`begin_pairwise`, `poll_pairwise`), `GroupCommand`, `AgreementCommand`,
`ReceiveCommand` (`begin_receive`, `poll_receive`) and `ValueCommand`
(EDCA, the BSS color, HE). A command in
progress is `InCommand<'id, H>`, typed by its kind: `H` holds the kind
and the rights its end returns, no value (the begin consumes the key, the
peer and the parameters). Its poll ends only
with that kind's end: the closed gate; `PairwiseEnd`, `GroupEnd` or
`AgreementEnd`, the slot always back, installed or empty (an expired key
command with its slot empty); `ReceiveEnd`, the rule's right always back,
`Receiving` or `Off`; `ValueEnd::Done` or `Expired`. Every end gives
the command's place back (`tests/ui/pairwise_end_is_not_a_gates.rs`).

## Receive as a round

`ReceivePort` (module `receive`) gives a round's primitives, each sealed
(`Sealed`, which only this crate makes: `tests/ui/sealed_round.rs`), and
`Round` is the contract's: it borrows the port, opens once
(`round_open`), services the ring once at its first `staged`, hands each
staged unit in ring order (`Unit::view`, `Unit::take`), names a taken unit
by its `UnitHandle` (`Round::unit`, `Round::lend`, `Round::release`) and
returns at its close every unit not taken (`round_close`), with what comes
next (`Rest`). A unit stays in the port's staging place from the ring's
service to its return and is read in place: its `RxHead`, classified once
at its staging, is a data or a management frame's decoded `RxHeader`, or a
control frame's Frame Control (a BlockAckReq), whose reader parses its
bytes; a frame that is neither is counted and goes back to the ring at its
staging, as a frame the air damaged.

## Vocabulary

The PHY and channel values come from
[`oer-protocols-ieee80211-wire`](../../wire/src/phy.rs): `Channel` and
`PhyRate`. A port's limits are its capabilities (`LowerMacCapabilities`,
`AmpduCapabilities` with its per-rate `AmpduLengthTable`), and a value
outside them has no checked type (`SupportedChannel`, `MpduLength`,
`SupportedBlockAck`, `SupportedEdca`). A single's description is a
`TxFrame` (an individual frame with its `RetryPlan`, a group frame with its
rate, a BlockAckReq), its header a `HeaderSpec` checked into a
`SizedFrame`; outcomes are `SingleOutcome` and `AggregateOutcome`.
`RxEvidence` and `RxCryptoStatus` describe receive provenance. The values a
backend and the layers above it share are in `contract`
(`MacServiceCapabilities`, `MacRxMetadata`, `MacTxStatus`), a VIF's EDCA
table is an `EdcaParameterSet`, the monitor boundary (`monitor`) carries
the tap point, the channel context, the observed frame and the injection
request. The access point's receive rules, TSF control and beacon timing
(`access_point`, `extensions`, `tsf`) stay for the access point's port.

## Implementers

The host model in `oer-protocols-ieee80211-testing-model` implements the
port (`ModelPort`) for the layouts it serves (`Served`: a station alone, an
access point alone) and runs the shared conformance suite
(`oer-protocols-ieee80211-testing-conformance`). No chip implements this
contract yet: the S31 backend has no port.
