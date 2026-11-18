# IEEE 802.11 data plane

`oer-protocols-ieee80211-core-datapath` is a sans-IO core. It forbids unsafe
code, has no runtime panic path and touches no port, frame buffer or clock of
its own: every instant and every arena is its caller's. It holds today:

- `engine`: one interface's engine over the port, the network's end
  (`Network`), the rate policy (`Rates`) and the controller's end
  (`Told`): its transmit half and its receive half (below). Its state is a sum: the data is `Stopped`, or flowing or
  stopping with what it holds while it runs (a station's plane built with
  its arenas, or an access point's lanes, the gate with the command's
  place, the rights), and a peer is free or linked with its link. A
  controller acts through views of these places (`DataPlace`, `PeerPlace`,
  `GatePlace`, and on a station's linked peer `KeyPlace`), whose handles
  offer only the transitions their variant admits; the rights come in by
  value at `flows` and go back with `DataStop::Stopped`. The command place
  is one sum (`Interface`: idle, a key's command, the gate's closing); the
  station's key is `StationKey { Empty, Installed(port key, transmit key) }`;
  a key's command and the gate's closing stop the intake;
- `plane`: the station's data transmission over the port's two data arenas;
- `numbers`: the link's transmit numbers (`TxNumbers`: each TID's QoS
  sequence space and the station's own space, which management frames and
  non-QoS data share; the pairwise key's transmit half with its CCMP packet
  numbers, `TxKey`, is held beside them), which the plane draws a data
  MPDU's from and the link's
  holder a station frame's (`oer-protocols-ieee80211-core-roles-sta`'s
  `link::number`);
- `queues`: the MAC's transmit queues (`TxQueues`), one per radio peer and
  TID over one bounded pool of the network's MSDUs;
- `events`: what the plane reports (`DataEvent`: a lost MSDU or MPDU with
  its reason, `DataLost`, and how an aggregate settled, `AggregateSettled`)
  to the `&mut impl EventSink<DataEvent>` its caller lends `completed`,
  `stop` and `withdraw_unpublished`;
- `cost`: what one attempt of the network's data costs on the air
  (`single_cost`, `aggregate_cost`): each transmission's PPDU at the rate
  it went at, one SIFS and the response at the lowest mandatory rate of its
  modulation class; nothing for a transmission that never reached the air,
  unknown where a rate cannot be timed or the end is not known;
- the A-MPDU limits (`PpduLimit`: the port's subframes, the lower of the
  access point's Maximum A-MPDU Length and the port's length at the
  aggregate's rate, and the rate's PPDU time limit,
  `PhyRate::max_txtime_micros`) and the retry policy (`AggregateRetry`)
  under the station's transmit agreements, and where the network places an
  MSDU in its memory: its Ethernet header the headroom's last octets
  (`MSDU_ETHERNET_OFFSET`), its payload after the headroom
  (`MSDU_PAYLOAD_OFFSET`, the contract's `HEADROOM`).

The engine's receive half. Each section that touches received units
(`Engine::received` at the ring's signal, `returned` when the network gave
lent units back, `due` at a waiting unit's bound or the ring's
continuation, a link's hold or release, and the data's stop) opens one
round over the port's receive (`Round`) and closes it. A unit stays in the
port's staging place from the ring's service to its return; the engine
reads it there and routes it: a management frame and EAPOL to the
controller, read by reference inside the round (`Told::frame`); a data
frame of a linked peer to the network; anything else back to the ring at
the round's close. The ring is read in every round: a management frame,
EAPOL and a BlockAckReq pass at once, whatever waits for the network. A
data unit for the network is decapsulated in its place once (its Ethernet
addresses written before the EtherType of its LLC/SNAP header,
`Round::write_addresses`), then lent to the network while it has room to
borrow (`Network::room`, `Network::lent`), else copied into the network's
storage while it has room for a copy (`Network::copied`; an A-MSDU's
MSDUs are copied, at most `AMSDU_MSDUS`), else it waits, taken, behind
what already waits, in the order of the moment each began to wait. A
newcomer from the ring keeps a place (lent or waiting) only while a place
stays free after it, the one the control plane passes through: at the
last place it is copied, when nothing waits and the network can copy it,
or refused for no place to wait; a unit the reorder lets go waits, or is
lent, in the place it already holds. A unit that waited `late` (a policy
value the composition gives `Engine::new` beside its budget) from its own
moment is refused at the head, every late head in one round; a waiting
unit of a held link is refused when it reaches the head. A data frame of a
held link, protected data without the port's evidence of its integrity,
and data that does not decapsulate are refused; the refusals are counted
by reason (`FlowingData::refusals`): no place to wait and late each have
their own count. After the data's stop nothing is delivered: what waited
and what the ring stages go back unread, and the rights come home once
nothing waits; the units the network holds are the installation machine's
to wait for (`UnitsHome`). The rule is `design/engine-model-rx.py`'s, and
the crate's search over the real engine checks its properties by the same
numbers.

A station's receive agreements and their reorder are `upper-mac`'s owner
(`RxBlockAckStorage`), held in the running data in the place of the
agreements' slots, its frames the units' handles. An ADDBA Request or a
DELBA of the access point is the agreements', read in the round; an
agreement is added and ended by a command of the port, begun when no
other command runs (`Interface::Agreeing`; the data goes on), and its
response is told owed only once it runs (`Told::reply_owed`): the
controller sends what the linked peer's `rx_reply` holds and says
`sent`. A data unit under an agreement goes through the reorder: a frame
of a hole is retained while a place stays free beyond it (the retained,
the lent and the waiting with it leave one of the port's `STAGING`
places), else it is refused; what the reorder releases goes behind what
waits, and is refused while the controller holds the link. A
BlockAckReq moves the window; the gap's timer (`ReceivePolicy::gap`)
gives a hole up at `due`. A link's end and the data's stop end its
agreements: what the reorder retains goes back undelivered, and the stop
ends the agreements by their commands before the key and the gate. An
access point's receive agreements are phase D's: its banks stay free.

The station's data transmission over the port's two arenas.
`DataPlane::new` takes the installation's two idle arenas, and each book
holds its arena's right in its place: idle, open or on the air.
`DataPlane::link` takes one link's `DataLink` (header template,
`AcAdmission`, retry policy, HT limits and each TID's `TxWindow`) into its
`Linkage::Linked` variant, and `DataPlane::stop` ends it, after returning
the frames of the arenas not on the air; `DataPlane::withdraw_unpublished` returns them alone, before a
pairwise key changes. `DataPlane::intake` makes an MSDU one MPDU in place,
as the standby arena takes it (`DataPort::place`, `Place::write`): its
body stays in the network's memory, and its front (the header from the
template with the next sequence number of its TID, the CCMP header of the
key's next packet number, LLC/SNAP) is written into the port's headroom
flush against the body, from the Ethernet header the network left there
(`StaDataHeaderTemplate::write_before`, which cannot fail). The standby
belongs to one TID, holds consecutive sequence numbers inside its window,
within the TID's `PpduLimit` and the arena's slots; a TID without
agreement takes one frame. An MSDU the standby cannot take waits in the
network's queue (`Intake::Wait`).

`DataPlane::publication` puts one arena on the air at a time, as the
caller's rate plan gives its `Ppdu`: an arena holding frames from an
earlier publication first, then the standby; more frames than one as an
A-MPDU under the TID's agreement (`Ppdu::Aggregate`, with the access
point's spacing and the protection the BSS asks), the longest prefix its
rate admits going, else a single MPDU of the arena's first frame
(`Ppdu::Single`), so the frames of a TID whose agreement ended drain one
MPDU at a time. `DataPlane::completed` reads the arena's end in place and
follows section 4 of the contract's order: the end, then, when a BlockAck
acknowledged every frame the arena held, the other arena published at
once (`Settled::ahead`), then the settlement (`DataPort::settle`).
Acknowledged frames are delivered; missing ones of an A-MPDU are kept in
their places with the same numbers until their MSDU's lifetime or the
policy's attempts run out, then dropped; attempts are a frame's, and a
frame that did not go spent none. A single MPDU had the port's retry plan
and is never sent again: it is delivered or dropped, under an agreement
too. A drop under an agreement moves its window and owes a BlockAckReq
(`Settled::bar`, which `Station::tx_block_ack_request` takes); the end's
`RateLesson` teaches the rate selection, a single MPDU's response through
the caller's ACK evidence, and `Settled::cost` is what the attempt cost.

An arena of an ended link settles by releasing every frame, without
moving any window or teaching rate selection. Each intake draws from the
link's `TxNumbers`, which the link's holder keeps.

The data plane's publications and their retry stages follow the BSS's HE
TXOP Duration RTS Threshold: while the access point advertises one, every
individually addressed HE PPDU, an A-MPDU included, goes behind RTS/CTS
(`ProtectionPolicy::every_he_txop_protection`, no duration estimate); the
ERP, HT and length rules are not applied to data.
