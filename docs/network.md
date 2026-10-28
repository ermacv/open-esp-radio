# Network stack and Wi-Fi egress

The boundary between the IP stack and the radio. The radio owns peer and
physical execution state; the network adapter owns its packet storage and the
stack-facing contract. The application keeps credentials, the network stack,
DHCP and sockets; stack APIs never expose radio peer slots or airtime grants.

## The stack

One network implementation: the owner's forks of Embassy (`embassy-net`) and
Xarxa (`xarxa`, `xarxa-driver` and its packet pool), pinned by Git revision in
the manifests (`portable/embassy/net/Cargo.toml`). Each fork follows
upstream by periodic merges, never by rebase; the `UPSTREAM.md` at a fork's
root names the last merged upstream commit and the upstream changes it does
not take. The driver and socket APIs differ from the released smoltcp-based
Embassy; package name and version alone do not identify the contract, the
Git source and revision do.

- `embassy-net` is the multi-interface API: `Stack::new` takes the stack
  storage, a seed and the `PacketBufAllocator`; the Wi-Fi device is lent with
  `add_iface_borrowed`, and IP configuration is set on the returned interface.
- The forks add explicit RX/TX packet pools, packet-owner handoff to the
  driver, credit-return wakes, bounded polling and construction in resource
  storage. A UDP or raw send blocked on the device wakes when its interface
  has room, one blocked on an empty pool when the pool has a buffer again;
  the stack never busy-retries a pool.
- IPv4 EchoReplies refused by the device retain their packet owners in an
  eight-entry FIFO per interface. Returned TX credit drains older replies
  before new ingress; overflow drops the newest reply and increments
  `Statistics::icmpv4_echo_dropped`. Refusal counters count attempts, including
  retries, rather than lost owners. The retention bound holds with or without
  allocation support and covers W3's eight-owner RX pool.
- A UDP send completes when the stack hands the packet to the driver or
  queues it for neighbour resolution: API completion is not delivery on air.
- TCP enters listen state on the first poll of `accept()`; a server must poll
  `accept()` before it announces readiness and keep that same future through
  the connection.

## Owners

| Crate | Owns | Does not own |
| --- | --- | --- |
| `oer-network` (`protocols/network`) | logical interfaces, link state, frame and admission errors, `TransportFlow` | queues, allocator, executor, hardware |
| `oer-embassy-net` (`portable/embassy/net`) | the Xarxa `PacketBuf` handoff, destination/flow TX queues, stack wake registration, RX copying into packet pools | radio scheduling, DMA descriptors |
| `oer-ieee80211-datapath` | software/physical TX ownership traits, `TxQueues`, the airtime ledger | a concrete allocator, stack or executor |
| `oer-ieee80211-softmac` | the TX cost model (`tx_cost`) and work receipts (`tx_work`) | hardware |
| the radio runner (P2) | VIF/peer generations, power-save eligibility, rate and retry state, BA sessions, DMA descriptors, physical credits, terminal TX receipts | sockets, a second stack |

## TX path

```text
network stack (P1)
    builds a general-memory PacketBuf
    classifies Ethernet destination and transport headers once
    publishes into destination/flow queues over one owner pool
radio runner (P2)
    selects a destination, then claims only its software owners
    validates VIF, peer generation, TID and power-save eligibility
    reserves the whole SRAM batch before consuming any selected owner
    copies each selected frame once into DMA-visible storage
    publishes; returns SRAM credits after terminal completion
```

- **General memory and SRAM are separate resources.** Selection happens
  before SRAM admission. A failed submission returns the original
  `PacketBuf`; a failed batch reservation removes nothing from the source.
- **Epochs.** Queue entries carry the interface link epoch; AP-retained owners
  also carry the association generation established at radio admission.
  Ethernet destination metadata alone grants no association or security
  authority.
- **`TxQueues`** holds one owner pool and two levels of metadata: destination,
  then transport flow. Each take rotates among a destination's nonempty flow
  FIFOs; several flows give no extra destination turns and can fill the same
  aggregate. `pending_for` and readiness count the whole destination.
  Selection scans metadata, never packets. A free owner slot always admits a
  new destination or flow (the stack's `can_transmit()` is destination-blind);
  there is no per-peer or per-flow reservation. `next_head_after` gives the
  next destination in address order, wrapping.
- **Admission credit.** The queue depth bounds every admitted software owner,
  in adapter queues and held by the radio together. Dequeue moves the credit
  with the owner; packet destruction returns the pool slot before it releases
  admission and wakes a blocked producer. SRAM materialization consumes the
  software owner and its admission; physical credits stay taken until DMA
  completion.
- **`TransportFlow`** classifies Ethernet-II IPv4/IPv6 TCP and UDP by exact
  directional addresses, protocol and ports (IPv4 options respected).
  Fragmented IPv4 uses one coarse addresses/protocol FIFO, including the first
  fragment; IPv6 extension chains are not traversed (addresses plus initial
  next header); VLAN, other encapsulations and malformed headers use an opaque
  FIFO. These are scheduling hints, never acceptance or security decisions.
  Order holds within a FIFO, not across a fragmented and an unfragmented class.
  Packet round-robin is not byte or airtime fairness.

## Airtime ledger (`oer-ieee80211-datapath::airtime`)

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
- `with_observer` attaches a bounded synchronous observer of grants,
  settlements and cancellations; it must not log, await or re-enter.

## TX cost model (`oer-ieee80211-softmac`)

- `PpduTiming` includes preamble, SERVICE and tail bits, whole-symbol rounding
  and the 2.4 GHz signal extension; HT uses the exact bits per symbol, not
  rounded SGI rates. Supported: DSSS/CCK, ERP-OFDM, HT mixed format with one
  stream, BCC, no STBC. HE is counted in `unestimated_ppdus`, not
  approximated; a nonzero count makes totals partial.
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

## Physical storage contracts (`oer-ieee80211-datapath`)

- `SoftwareTxFrame`: an affine software owner and Ethernet view.
- `MaterializedTxFrame`: stable DMA ownership, Ethernet geometry and
  conservative capacity bounds.
- `PhysicalTxSource`: synchronous transfer of final physical owners.
- `SelectedBurstMaterializer`: reserve-before-remove single or batch promotion.
- `TxRequestSource`: admits a selected request without Ethernet bytes; a busy
  or failed start returns the original request.

Ordinary and aggregate service transitions are synchronous. A timeout abort
keeps the transaction with a settle deadline; storage is released only after
hardware detach, or quarantined if detach fails; early wakes and late
completions cannot bypass the settle interval.

## RX and wakes

- RX is copied into the endpoint's RX pool (`OwnedRxPublisher`) before
  publication. The network retains packet-pool storage independently of the
  radio's DMA storage, and the packet returns to its pool when dropped.
- Cross-domain wakes are hints over durable state: consumers check, arm and
  recheck before sleeping. An exhausted work budget self-wakes; real resource
  exhaustion waits for its credit-return edge. RX availability, general pool
  availability and SRAM completion are separate domains.

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
