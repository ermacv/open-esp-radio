# Network and Wi-Fi egress

The boundary between the network and the radio. The radio owns peer and
physical execution state; the network core owns its blocks, sockets and the
choice of work before a frame is built. The application supplies credentials,
an interface's addressing and socket workloads. TCP has no implementation:
the network core carries UDP, ARP, DHCP and ICMP echo.

## The network core

One network implementation: the network core (`oer-protocols-network-core`)
and its runtime (`oer-protocols-network-runtime`), over a role's port. A
network is an instance over one port: the image lends its storage
(`oer::wifi::NetworkStorage`: the stop's lane, the service's place, the
watch of its address, its channel socket's lanes), its blocks in a zeroed
static of their own (`oer::wifi::NetworkMemory`) and names its brand, a
type of its own, so nothing of the library makes a second network
impossible. `oer::wifi::Network::start` takes a role's device (the station's
or the access point's, the same `Device`) and its addressing: leased by the
core's DHCP client (`Addressing::Dhcp`), or given (`Addressing::Given`). The
networks an image starts run in its task group, together in one task: on
hart 1 with the `group` feature, otherwise on the application's executor
beside the roles' runners. A socket lives on its network's hart.

- A UDP send completes when the port accepts the datagram
  (`Outcome::Accepted`): API completion is not delivery on air.
- A network's stop answers its role's request: the service retires, its
  port gives back every frame it holds and takes back every memory the role
  returned, then it reports; after the report its task keeps the port and
  serves its returns for the rest of the boot, so a memory the role gives
  back late still goes home.

## Owners

| Crate | Owns | Does not own |
| --- | --- | --- |
| `oer-protocols-network-core` (`protocols/network/core`) | the sans-IO core with one owner: UDP, ARP, DHCP, ICMP echo, blocks, choosing work before a frame is built | tasks, queues, time |
| `oer-protocols-network-runtime` (`protocols/network/runtime`) | the core's task, its sockets and its local producer, and `DevicePort`, whose frames carry the buffers' tickets and whose notices are the buffers' ends | protocol state, the port's implementation |
| `oer-protocols-network-testing-model` (`protocols/network/testing/model`) | a host model of one shared medium with several peers, the transmit scenarios over the real stack, and the end-to-end station and network simulation against a simulated access point (its [README](../protocols/network/testing/model/README.md)) | product code; no product package depends on it |
| `oer-protocols-network-port` (`protocols/network/port`) | link state, frame and admission errors, the `NetDevice` contract, and `FramePort`, `FrameSource` and `OwnedTransmit`, the core's port | queues, allocator, executor, hardware |
| `oer-protocols-network-wire` (`protocols/network/wire`) | Ethernet II, VLAN, IPv4, IPv6, TCP and UDP header fields; its one dependency is `oer-portable-wire` | decisions about a packet, IEEE 802.11 formats |
| `oer-protocols-bridges-ieee80211-ethernet` (`protocols/bridges/ieee80211-ethernet`) | the requested WMM user priority of an outgoing frame from its VLAN PCP or IP DSCP | the final priority, which the station decides from its link |
| `oer-protocols-ieee80211-runtime` (`station`, `network_return`) | a role's network end: `Device`, the transmit owner `LocalTx`, the `NetworkLink` and the runner's `NetworkEnds`, with the memory's way home and the network's stop (`NetworkReturn`), shared by the station's actor and the access point's runner | the network's protocol state |
| `oer-chips-esp32s31-radio-ieee80211-backend::mac` | the TX cost model (`mac::tx_cost`) and work receipts (`mac::tx_work`) of its publications | packet storage |
| the radio runner (P2) | VIF/peer generations, power-save eligibility, rate and retry state, BA sessions, DMA descriptors, physical credits, terminal TX receipts | sockets |

## TX path

```text
network core (P1)
    chooses ready work; the port grants a transmit buffer of the role
transmit owner LocalTx (P1, the same call)
    the core writes the Ethernet frame in that buffer, at its start
    publishes the buffer to the role's runner
station's actor or access point's runner (P0)
    writes the 802.11 header in front of the payload, in place
    publishes the same buffer; the buffer returns to P1 after its terminal
```

- **The network's buffers are the role's transmit buffers.** The transmit
  owner (`LocalTx`) holds every buffer the port does not hold; a frame is
  written where the radio sends it from. The role's transmit queue is the
  one queue on the way: a full one, or no free buffer, refuses before the
  frame is written, and the work stays ready until room or a buffer comes.
- **A burst wakes the runner once.** The owner publishes a burst's frames
  and flushes the queue at the burst's end, which tells a waiting runner
  once, so the burst goes out as one aggregate.

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

## Transmit on the station: the MAC's queues

The network chooses no air. Its service publishes the ready work the core
lists (`oer-protocols-network-runtime`'s README, "Publishing"), and a frame
the port takes goes to the station's MAC, which decides when it goes and
charges its air. How an application provides data, a local producer or a
socket's blocks, does not change this; a frame is built in a transmit buffer
only once the port gave the room.

- **Queues.** The data plane (`oer-protocols-ieee80211-runtime`,
  `datapath::queues`) keeps one logical queue per radio peer and TID over
  one bounded pool of the network's MSDUs, which the network's buffers
  bound. The station has one peer, its access point; the access point's
  runner keeps a queue per authorized client and one for the group. A
  client place's queues belong to one association
  (`ApAssociationIdentity`, its address, AID and epoch): a later
  association of the place takes them over and the earlier one's frames go
  back to the network, a frame goes only while its association is current
  (`association_is_current`, checked before it is written), and a
  completion charges its queue only while its own association owns it.
- **Choice.** The next transmission starts from the highest access category
  any ready queue holds; of the peers with a ready queue of it, the one that
  had the least air; of that peer's queues of it, the one that had the least
  air. Several TIDs of one peer earn it no more air than one. A transmission
  of one queue is the quantum: an arena holds one TID, so a standby that
  holds frames takes from its own queue only. With exactly one ready queue
  the choice reads no other.
- **Blocking.** A queue is ready while it holds an MSDU and waits for
  nothing. A queue that waits (for its Block Ack window, a free arena or the
  standby) blocks only itself: another TID of the same access category and
  every other queue go on, and the waiting queue keeps its own MSDUs.
- **Cost.** Each arena attempt that ended is charged once, whole, to its
  queue and its peer: `TxEstimate`, the cost of its publications that
  reached the air by the backend's declared model, the modelled PPDU
  (`PpduTiming`), one SIFS and the solicited Ack or BlockAck at the lowest
  mandatory rate of the PPDU's modulation class. AIFS, backoff and CCA
  waits are not counted, nor publications that never reached the air. It is
  the cost of that model, not a measured medium occupancy. An attempt the
  model cannot time is charged the dearest attempt known, at least one
  microsecond: an unknown cost is never zero. The access point's runner
  charges each data attempt what its single MPDU's terminal carries
  (`Completed::cost`): the S31 backend sums, over the plan's publications,
  the work each added to its descriptor's record by the same model, none
  for a publication that did not reach the air, and `Unknown` where the
  model cannot say (a hardware timeout, a refused key, an unanswered
  protection exchange). The reference model declares single MPDUs'
  costs unknown.
- **Return.** A frame's buffer comes back to the network's one transmit
  owner (`LocalTx`) when its MSDU ends, and is free again at once: no
  separate settlement follows. A new link epoch or a stopped data plane
  hands every queued MSDU back with its memory.
- **The access point's data.** The access point's runner takes the
  station's network end, whole: its network memory is the same S31 transmit
  pool (`NetworkTx`: every place its port bank does not hold), held by the
  same owner, and its frames go through the port's single-MPDU path from
  that memory (`TxNetworkPool`): no copy into a port buffer. The network
  places the payload where the longest data prefix ends
  (`MSDU_PAYLOAD_OFFSET` of the frame region, the S31 region behind its
  8-octet transmit metadata); the runner checks the frame, its authorized
  destination, its key and a packet number left, its rate and power, then
  draws the sequence number as the engine writes the MAC header, a reserved
  CCMP header and LLC/SNAP in front of the payload, in place, and only then
  the packet number. A QoS peer's frame moves nothing; a 24-octet header (a
  group frame, a non-QoS peer's) moves the payload down two octets once,
  counted in `ApStatus::payload_moves`. A refusal returns the memory to the
  network unwritten, as the MPDU's terminal does: each memory waits in the
  runner's way home (`NetworkReturn`, which holds `AP_TX_DEPTH` memories, at
  least every network buffer by a compile-time check) until the return
  queue takes it, so no give-back is refused; a stop or a client place's
  takeover returns every frame still queued. A refusal after the sequence
  number was drawn, which the checks exclude, is the runner's named contract
  break (`ApContract::NetworkAdmission`). In the S31 backend a buffer's
  memory and the right that takes it home (its bank place, or the network)
  are one lease (`TxLease`): the queue's descriptor takes it whole at
  publication and gives it back whole at completion, and no memory is
  matched to a home afterwards. A lease its buffer refuses back stays whole
  on the descriptor with its attempt beside it and what is known of the
  attempt (`Fault::ReturnRefused`, which lets a stop recover); after the
  confirmed stop the cleanup (`clean_stopped`) gives it back to that
  attempt, which ends with its known result: a delivery stays delivered,
  one never published is withdrawn. A lease whose attempt is gone
  (`Fault::CompletionWithoutPending`) goes home by its own home once the
  completion released it: a bank's to its place at once, a network's to the
  port's owner (`AccessPointPort::take_network_memory`), which the runner
  gives back to the network whenever it returns the frames a halt or a stop
  left.
- **The access point's stop.** The station's handshake: once the radio's
  stop is confirmed, every network memory the runner or the port holds goes
  home, the network is asked to stop, and its report settles the stop within
  the policy's deadline (`ApPolicy::network_stop`); a frame the network
  published before it saw the request is taken at the report. The outcomes
  are the station's: confirmed, `NetworkLost` once the runner's share is
  home, or `NetworkReturning`, after which the stop's watch goes on.

## RX and wakes

- A socket's receive mode is explicit at its bind
  (`oer::wifi::Copying` or `oer::wifi::Lending<CREDIT>`). A lending socket
  keeps a datagram in the station's receive buffer under a loan of its
  credit. The station's lend count holds `LENT_RX` buffers; the account gives
  the socket `CREDIT` of them and keeps the rest for frames on their way, a
  const check holding `CREDIT` to that count. The station marks each copy
  with its reason: by form (an A-MSDU, or an MSDU sharing its frame with the
  EAPOL; the access point's every MSDU, as its role lends no buffer), which
  a lending socket takes in a block of its group, or by
  pressure (a backend that lends nothing, as when it stops), which it
  drops. With every lendable buffer out
  the station stops its pull and the frames wait in the ring, so a slow
  network paces the peer through unacknowledged ring slots. Such a
  pressure episode lasts at most the runtime's `NETWORK_LATE` from its
  first stop, whatever the network returns meanwhile; then the station
  walks the ring to its end, management, EAPOL and Block Ack frames are
  served, and an MSDU the port cannot lend is refused, counted as handoff
  pressure (`RxMsdusRefused::pressure`), its buffer back at once. This
  removes the software barrier only: a buffer the network holds keeps its
  descriptor, and the ring rearms in order behind it, so a loan held while
  the ring comes round still stops reception until it returns. A buffer's loan frees when its
  memory returns, never at a stop; the station's stop confirms once the
  network stopped, without waiting for lent buffers, which rejoin whichever
  ring runs when they drop.
- A turn's received frames wake the network once, at the end of the turn's
  receive stage; a frame the network takes tells the station only while it
  waits for room in that queue.
- Cross-domain wakes are hints over durable state: consumers check, arm and
  recheck before sleeping. An exhausted work budget self-wakes; real resource
  exhaustion waits for its return edge. RX availability and general
  pool availability (transmit buffers returned) are separate domains.
