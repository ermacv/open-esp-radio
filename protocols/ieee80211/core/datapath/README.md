# IEEE 802.11 data plane

`oer-protocols-ieee80211-core-datapath` is a sans-IO core. It forbids unsafe
code, has no runtime panic path and touches no port, frame buffer or clock of
its own: every instant and every arena is its caller's. It holds today:

- `plane`: the station's data transmission over the port's two data arenas;
- `numbers`: the link's transmit numbers (`TxNumbers`: each TID's QoS
  sequence space, the station's own space, which management frames and
  non-QoS data share, and the pairwise key's transmit half with its CCMP
  packet numbers), which the plane draws a data MPDU's from and the link's
  holder a station frame's (`oer-protocols-ieee80211-core-roles-sta`'s
  `link::number`);
- `queues`: the MAC's transmit queues (`TxQueues`), one per radio peer and
  TID over one bounded pool of the network's MSDUs;
- `events`: what the plane reports (`DataEvent`: a lost MSDU or MPDU with
  its reason, `DataLost`, and how an aggregate settled, `AggregateSettled`)
  to the `&mut impl EventSink<DataEvent>` its caller lends `completed`,
  `stop` and `withdraw_unpublished`;
- the A-MPDU limits (`PpduLimit`: the port's subframes, the lower of the
  access point's Maximum A-MPDU Length and the port's length at the
  aggregate's rate, and the rate's PPDU time limit,
  `PhyRate::max_txtime_micros`) and the retry policy (`AggregateRetry`)
  under the station's transmit agreements, and where an MSDU sits in a frame
  region (`MSDU_ETHERNET_OFFSET`).

The station's data transmission over the port's two DMA
arenas (`TxArena`). `DataPlane::handover` takes one link's `DataLink`
(from the `LinkHandover`: header template, `AcAdmission`, retry policy,
HT limits and each TID's `TxWindow`) and `DataPlane::stop` returns it,
after returning the frames of unpublished arenas;
`DataPlane::withdraw_unpublished` returns them alone, before a pairwise
key changes. `DataPlane::intake` writes an MSDU once, as the standby
arena takes it: the next sequence number of its TID and packet number of
the key, drawn from the link's `TxNumbers`, which the station's own EAPOL
and Block Ack negotiation share, the header from the template
and the CCMP header. The standby belongs to one TID, holds
consecutive sequence numbers inside its window, within the TID's
`PpduLimit` (computed when the aggregate rate or the agreement changes)
and the arena's slots; a TID without agreement takes one frame. An MSDU
the standby cannot take waits in the network's queue (`Intake::Wait`).
`DataPlane::publication` puts one arena on the air at a time, one rate
plan each: an arena holding frames from an earlier publication first,
then the standby; more frames than one as an A-MPDU under the TID's
agreement, else a single MPDU of the arena's position 0, so the frames
of a TID whose agreement ended drain one MPDU at a time.
`DataPlane::completed` decodes an A-MPDU's BlockAck in one pass over the
arena's frames: acknowledged frames are delivered, missing ones retried
in the same arena with the same numbers until their MSDU's lifetime or
the policy's attempts run out, then dropped. A single MPDU had the port's
retry plan and is never republished: it is delivered or dropped, and the
arena keeps the frames it did not send. A drop under an agreement moves
its window and owes a BlockAckReq (`Settled::bar`, which
`Station::tx_block_ack_request` takes), and the end's `RateLesson` teaches
the rate selection, a single MPDU's response through the caller's ACK
evidence;

The data plane keeps each arena's frames in a book beside its arena right:
their TID, first sequence number, admission instants and the attempts on
the air; the arena holds their memory. An arena of an ended link settles
by releasing every frame, without moving any window or teaching rate
selection. Each intake draws from the link's `TxNumbers`, which the
link's holder keeps.

The data plane's publications and their retry stages follow the BSS's HE
TXOP Duration RTS Threshold: while the access point advertises one, every
individually addressed HE PPDU, an A-MPDU included, goes behind RTS/CTS
(`ProtectionPolicy::every_he_txop_protection`, no duration estimate); the
ERP, HT and length rules are not applied to data.
