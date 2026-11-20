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
impossible. `oer::wifi::Network::start` takes a role's device and its addressing: leased by the
core's DHCP client (`Addressing::Dhcp`), or given (`Addressing::Given`). The
networks an image starts run in its task group, together in one task: on
hart 1 with the `group` feature, otherwise on the application's
executor. A socket lives on its network's hart.

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
| `oer-chips-esp32s31-radio-ieee80211-backend::mac` (sources outside the build) | the TX cost model (`mac::tx_cost`) and work receipts (`mac::tx_work`) of its publications | packet storage |
| the radio runner (P2) | VIF/peer generations, power-save eligibility, rate and retry state, BA sessions, DMA descriptors, physical credits, terminal TX receipts | sockets |

## TX cost model (`oer-chips-esp32s31-radio-ieee80211-backend::mac`)

The model lives in the old Wi-Fi backend, a source outside the build.

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

## Transmit memory in the S31 backend

- **Cost of an attempt.** An arena attempt's `TxEstimate` is the cost of its
  publications that reached the air by the backend's declared model: the
  modelled PPDU (`PpduTiming`), one SIFS and the solicited Ack or BlockAck
  at the lowest mandatory rate of the PPDU's modulation class. AIFS,
  backoff and CCA waits are not counted, nor publications that never
  reached the air. It is the cost of that model, not a measured medium
  occupancy. A single MPDU's terminal carries its cost (`Completed::cost`):
  the S31 backend sums, over the plan's publications, the work each added
  to its descriptor's record by the same model, none for a publication that
  did not reach the air, and `Unknown` where the model cannot say (a
  hardware timeout, a refused key, an unanswered protection exchange). The
  reference model declares single MPDUs' costs unknown.
- **Leases.** In the S31 backend a buffer's memory and the right that takes
  it home (its bank place, or the network) are one lease (`TxLease`): the
  queue's descriptor takes it whole at publication and gives it back whole
  at completion, and no memory is matched to a home afterwards. A lease its
  buffer refuses back stays whole on the descriptor with its attempt beside
  it and what is known of the attempt (`Fault::ReturnRefused`, which lets a
  stop recover); after the confirmed stop the cleanup (`clean`) gives it
  back to that attempt, which ends with its known result: a delivery stays
  delivered, one never published is withdrawn. A lease whose attempt is
  gone (`Fault::CompletionWithoutPending`) goes home by its own home once
  the completion released it: a bank's to its place at once, a network's to
  the port's owner (`AccessPointPort::take_network_memory`).

## RX and wakes

- A socket's receive mode is explicit at its bind
  (`oer::wifi::Copying` or `oer::wifi::Lending<CREDIT>`). A lending socket
  keeps a datagram in the station's receive buffer under a loan of its
  credit.
- Cross-domain wakes are hints over durable state: consumers check, arm and
  recheck before sleeping. An exhausted work budget self-wakes; real resource
  exhaustion waits for its return edge. RX availability and general
  pool availability (transmit buffers returned) are separate domains.
