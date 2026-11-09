# Network stack and Wi-Fi egress

The boundary between the IP stack and the radio. The radio owns peer and
physical execution state; the network adapter owns its packet storage and the
stack-facing contract. The application supplies credentials, IP configuration
and socket workloads. `oer_portable_embassy_net::ip` constructs its pools, stack and
Ethernet bridge once and owns DHCP/static configuration and product IP status;
stack APIs never expose radio peer slots or airtime grants.

## The stack

One network implementation: the owner's forks of Embassy (`embassy-net`) and
Xarxa (`xarxa`, `xarxa-driver` and its packet pool), pinned by Git revision in
the manifests (`portable/embassy/net/Cargo.toml`). Each fork follows
upstream by periodic merges, never by rebase; the `UPSTREAM.md` at a fork's
root names the last merged upstream commit and the upstream changes it does
not take. The driver and socket APIs differ from the released smoltcp-based
Embassy; package name and version alone do not identify the contract, the
Git source and revision do.

Direct normal dependencies on `embassy-net`, `xarxa` and `xarxa-driver`
belong only to `portable/embassy/net` and firmware. The layer tests resolve
aliases and include optional and target-specific dependencies.

- `embassy-net` is the multi-interface API: `Stack::new` takes the stack
  storage, a seed and the `PacketBufAllocator`; the Wi-Fi device is lent with
  `add_iface_borrowed`, and IP configuration is set on the returned interface.
- The forks add explicit RX/TX packet pools, packet-owner handoff to the
  driver, a full device's room wake, bounded polling and construction in resource
  storage. A UDP or raw send blocked on the device wakes when its interface
  has room, one blocked on an empty pool when the pool has a buffer again;
  the stack never busy-retries a pool.
- A UDP send completes when the stack hands the packet to the driver or
  queues it for neighbour resolution: API completion is not delivery on air.
- TCP enters listen state on the first poll of `accept()`; a server must poll
  `accept()` before it announces readiness and keep that same future through
  the connection.

## Owners

| Crate | Owns | Does not own |
| --- | --- | --- |
| `oer-protocols-network-core` (`protocols/network/core`) | the sans-IO core with one owner: UDP, ARP, blocks, choosing work before a frame is built; the HIL `wifi-station` image links it | tasks, queues, time |
| `oer-protocols-network-runtime` (`protocols/network/runtime`) | the core's task, its sockets and its local producer, and `DevicePort`, whose frames carry the buffers' tickets and whose notices are the buffers' ends; the HIL `wifi-station` image links it | protocol state, the port's implementation |
| `oer-protocols-network-testing-model` (`protocols/network/testing/model`) | a host model of one shared medium with several peers, the egress scenarios over the real stack, and the end-to-end station and network simulation against a simulated access point (its [README](../protocols/network/testing/model/README.md)) | product code; no product package depends on it |
| `oer-protocols-network-port` (`protocols/network/port`) | logical interfaces, link state, frame and admission errors, the `NetDevice` and `NetTransmit` contracts, and `FramePort`, `FrameSource` and `Egress`, the new core's port and its policy | queues, allocator, executor, hardware |
| `oer-protocols-network-wire` (`protocols/network/wire`) | Ethernet II, VLAN, IPv4, IPv6, TCP and UDP header fields; its one dependency is `oer-portable-wire` | decisions about a packet, IEEE 802.11 formats |
| `oer-protocols-bridges-ieee80211-ethernet` (`protocols/bridges/ieee80211-ethernet`) | the requested WMM user priority of an outgoing frame from its VLAN PCP or IP DSCP | the final priority, which the station decides from its link |
| `oer-portable-embassy-net` (`portable/embassy/net`) | the Xarxa `PacketBuf` handoff, stack wake registration, RX copying into packet pools, a device's transmit memory parked as the stack's general packets (`ParkedTx`) | radio scheduling, DMA descriptors, a concrete physical allocator |
| `oer-portable-embassy-net::ip` | final pool/stack storage, one `EthernetBridge` over `NetDevice`, DHCP/static IPv4, link epoch and typed configuration/stop/error status | radio installation, a second stack, a common TCP/UDP API |
| `oer-chips-esp32s31-radio-ieee80211-backend::mac` | the TX cost model (`mac::tx_cost`) and work receipts (`mac::tx_work`) of its publications | packet storage |
| `oer-protocols-ieee80211-core-upper-mac` | the airtime ledger (`airtime`) and affine scheduling reservations | hardware, packet storage |
| the radio runner (P2) | VIF/peer generations, power-save eligibility, rate and retry state, BA sessions, DMA descriptors, physical credits, terminal TX receipts | sockets, a second stack |

## TX path

```text
network stack (P1)
    builds a PacketBuf in a station transmit buffer (its general allocator)
    hands it to the network end, in the call that sent it
network end (P1, the same call)
    takes the buffer out of the packet: the frame stays where it was written
    copies a frame from elsewhere (a reply in a received buffer) into its spare
    publishes the buffer to the station
station (P2)
    writes the 802.11 header in front of the payload, in place
    publishes the same buffer; the buffer returns to P1 after its terminal
```

- **The stack's general packets are the station's transmit buffers.** The
  network end parks every buffer the port does not hold in `TxSlots`
  (`oer-portable-embassy-net::ethernet`), whose allocator is the stack's
  general allocator; a dropped packet parks its buffer again. The station's
  transmit queue is the one queue on the way: a full one refuses the original
  `PacketBuf` to the stack, which keeps it and waits for the room wake.
- **A burst wakes the station once.** A frame that finds the station asleep
  and waiting for its first frame leaves it asleep; the sender's frame wakes
  the stack's poll task on the network's executor, which runs once the sender
  yields and tells the station the burst ended, so the burst goes out as one
  aggregate. A station waiting for several frames wakes at that count.
## Airtime ledger (`oer-protocols-ieee80211-core-upper-mac::airtime`)

- `AirtimeStorage` holds bounded per-peer deficit accounts apart from packet
  storage; `scheduler()` borrows it exclusively. Tokens of one ledger cannot
  settle another's (identity check by reference; no allocation, lock, global
  counter or `unsafe`).
- The key is the peer with its association generation, not the flow;
  duplicate candidates get one share. Idle or sleeping peers earn no credit,
  lose positive surplus and keep debt. Exhausted rounds advance arithmetically,
  with no timer or wake.
- `reserve` deducts a grant before preparation; an unpublished reservation can
  be cancelled; publication turns it into `AirtimeInFlight`, which retries
  keep; after terminal detach `completed` joins receipt and reservation;
  `settle` needs an explicit nonzero modelled cost. Unknown cost is never zero
  or a refund; dropping a token refunds nothing. Retiring a generation pins it
  while work is outstanding.
- `reserve`, `settle` and `cancel` report each grant, settlement and
  cancellation to the caller's `EventSink`; the ledger holds no sink.

## TX cost model (`oer-chips-esp32s31-radio-ieee80211-backend::mac`)

- `PpduTiming` includes preamble, SERVICE and tail bits, whole-symbol rounding
  and the 2.4 GHz signal extension; HT uses the exact bits per symbol, not
  rounded SGI rates. Supported: DSSS/CCK, ERP-OFDM, HT mixed format with one
  stream, BCC, no STBC; HE20 SU with one stream, MCS 0-9, every GI/HE-LTF
  and BCC or LDPC, from the APEP length through pre-FEC padding, the LDPC
  extra symbol segment and the packet extension. The HE packet extension is
  bounded by a 16-us nominal padding, since the backend does not program the
  peer's. HE DCM is counted in `unestimated_ppdus`, not approximated; a
  nonzero count makes totals partial.
- `TxCost` separates data PPDU, SIFS plus ACK or BlockAck, CTS-to-self or
  RTS/CTS, and AIFS plus selected backoff. No response is an explicit zero; an
  unknown response rate or missing access facts stay `None`; overflow yields
  no total. `exchange_micros` excludes contention; `service_micros` adds the
  selected backoff but not CCA freezes, CPU service or hidden hardware
  retries. `maximum_psdu_bytes` inverts the model in integer arithmetic and
  returns `None` rather than an unrestricted grant.
- `MacTxWork` receipts sum each publication's own PSDU length and rate
  (selective retries and rate fallback charge their real geometry),
  programmed AIFSN (`aifs_slots`) and selected backoff; `unreported_contention`
  counts publications without them. These are programming facts, not observed
  channel waiting. On the S31 the selector is `EdcaBackoffState::select_slot`:
  the published `contention_window` value is the selected count, not the
  window maximum.
- S31 `HtRate::ampdu_exchange_byte_limit` bounds one unprotected HT aggregate
  plus compressed BlockAck under caller-supplied response timing, intersected
  with the peer's byte ceiling and the vendor rate limit. The ACK/BlockAck rate
  is an explicit assumption: the RTS-rate field does not establish it, and the
  local ACK-rate registers describe responses the ESP sends, not the peer's.
  `HeRate::checked_maximum_apep_bytes` is the vendor TXOP admission bound with
  an estimated BlockAck overhead, not an HE duration model.
- `TxCompletion` exposes completion class, ACK SNR and trigger-flow packet
  counts (not retry counts); it exposes no response PHY or measured airtime.

## Queued egress on the station (`oer::wifi::Queued`)

The network core's egress is chosen by type: `wifi::Network<P, Queued>` takes
the airtime egress (`AirtimeEgress`), `wifi::Network<P, Direct>` the first
ready work with no table or account. How an application provides data, a
local producer or a socket's blocks, does not depend on the mode; waiting work
stays outside the transmit buffers in either, and a frame is built in one
only after the policy chose it and the port gave the room.

- **Accounts.** `StationAir` (`oer-protocols-ieee80211-runtime::station`)
  keys the station's air by the link epoch: every frame of a station,
  broadcast included, goes to its access point, so one account per
  association. Control work goes first; flows take turns within the
  account. Service classes are not separated yet.
- **Choice.** A choice reads the ready work, the network's whole backlog,
  once: each work's least grant once, kept per account as the largest and
  with the work next in turn, which is all the scheduler and the pick read
  of it. `StationAir` keeps the last frame length's least grant and
  ceiling, as a backlog is mostly of one length. The scan stays one pass
  over the backlog per frame sent.
- **Cost.** Each arena attempt reports `TxEstimate`, the cost of its
  publications that reached the air by the backend's declared model: the
  modelled PPDU (`PpduTiming`), one SIFS and the solicited Ack or BlockAck
  at the lowest mandatory rate of the PPDU's modulation class. AIFS, backoff
  and CCA waits are not counted, nor publications that never reached the air.
  It is the cost of that model, "PPDU + SIFS + ACK/BA", not a measured medium
  occupancy and not a frame's service time. That the S31 MAC makes no
  transmission of its own inside one publication is an unverified
  assumption; it limits how far the model follows the air, and no airtime
  guarantee rests on it.
- **Split.** The data plane divides each attempt's cost among the frames the
  attempt carried, exactly once, the first taking the remainder, and keeps
  each frame's sum across its selective retries; a frame returns its memory
  with that sum. An attempt the model cannot time makes its frames' cost
  `Unknown` for good.
- **Ceiling.** A frame of unknown cost is charged
  `msdu_cost_ceiling_micros` (`lower_mac::arena`): up to the data plane's 32
  aggregate attempts of a member, each its whole aggregate (HT within its
  5484 us L-SIG TXTIME, HE the longest PPDU of the HE aggregate limit at HE
  MCS 0, 4x HE-LTF, 3.2 us GI), then the largest single plan of any rate
  record (32) at 1 Mb/s DSSS. It is a margin, not a statement of accuracy.
- **Return and settlement are two events.** The station's buffer comes back
  with its frame's cost (`TxSupply::give`) and waits with that end in
  `Completions`; the buffer is free again only when the port hands the end
  on to the policy (`TxBuffers::take_notice`). So no more frames are
  unsettled than there are buffers, which is the policy's reservation
  bound (`NETWORK_BUFFERS`). The port asks for returned buffers whether or
  not it sends, so an idle network still settles. A frame lost with its
  buffer at publication settles as `RadioNotice::Lost` at its ceiling; that
  settles the account only and returns no memory. A new link epoch ends
  the old association's account (`RadioNotice::Left`) before the next end:
  each of its frames still out settles there once, at its ceiling, and the
  account goes at once; the frame's buffer stays the port's until it comes
  back, and its return only closes the ticket, with no second charge and
  nothing on the new account. An outcome a socket already read is not
  changed by this settlement.

## Physical storage contracts (`oer-portable-embassy-net::datapath`)

- `SoftwareTxFrame`: an affine software owner and Ethernet view.
- `MaterializedTxFrame`: stable DMA ownership, Ethernet geometry and
  conservative capacity bounds.
- `PhysicalTxSource`: synchronous transfer of final physical owners.
- `SelectedBurstMaterializer`: reserve-before-remove single or batch promotion.
- `TxRequestSource`: admits a selected request without Ethernet bytes; a busy
  or failed start returns the original request.

Ordinary and aggregate service transitions are synchronous. A timeout abort
keeps the transaction with a settle deadline; storage is released only after
hardware detach or the completion of a PPDU the abort found on the air, or
quarantined if detach fails; early wakes and late completions cannot bypass
the settle interval.

## RX and wakes

- Over Xarxa, RX is copied into the endpoint's RX pool (`OwnedRxPublisher`)
  before publication. The network retains packet-pool storage independently
  of the radio's DMA storage, and the packet returns to its pool when dropped.
- Over the network core, a socket's receive mode is explicit at its bind
  (`oer::wifi::Copying` or `oer::wifi::Lending<CREDIT>`). A lending socket
  keeps a datagram in the station's receive buffer under a loan of its
  credit. The station's lend count holds `LENT_RX` buffers; the account gives
  the socket `CREDIT` of them and keeps the rest for frames on their way, a
  const check holding `CREDIT` to that count. The station marks each copy
  with its reason: by form (an A-MSDU, or an MSDU sharing its frame with the
  EAPOL), which a lending socket takes in a block of its group, or by
  pressure (no lend room), which it drops. A buffer's loan frees when its
  memory returns, never at a stop; the station's stop confirms once the
  network stopped, without waiting for lent buffers, which rejoin whichever
  ring runs when they drop.
- A turn's received frames wake the network once, at the end of the turn's
  receive stage; a frame the network takes tells the station only while it
  waits for room in that queue.
- Cross-domain wakes are hints over durable state: consumers check, arm and
  recheck before sleeping. An exhausted work budget self-wakes; real resource
  exhaustion waits for its return edge. RX availability and general
  pool availability (transmit buffers parked again) are separate domains.

## Access point egress

Rules for an access point serving several stations with interleaved
destinations:

- Retained active frames, awake unicast power-save backlog and producer queues
  share one destination cursor: a destination present in several gets one
  turn; its buffered PS prefix goes before active retention, which goes before
  newer source packets. PS-Poll keeps its single-frame release and DTIM its
  group release, outside that cursor.
- Active retention, unicast PS and DTIM group queues share one bounded owner
  arena outside the movable AP future, borrowed for one AP epoch. A PS release
  keeps its packet in its slot through terminal TX, so rollback needs no fresh
  storage; success frees slot and owner exactly once. Endpoint admission is
  sized to the arena, so every admitted owner can move into PS storage. A
  dropped PS frame is never counted in the TIM.
- Stale association generations release their owners before selection;
  power-save is rechecked before encoding; a sleep transition moves retained
  frames to PS storage and updates the TIM; a peer that sleeps after
  selection returns its unpublished grant.
- Airtime accounting, an HT aggregate byte ceiling from the grant and
  deficit-based destination selection are optional modes, off by default;
  round-robin by destination is the default. Equal throughput or full
  aggregates do not show airtime fairness when peers use different rates.
- An access point that advertises only CCK rates as basic makes the generic
  highest-basic-rate rule an invalid HT BlockAck timing assumption.
