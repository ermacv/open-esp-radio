# ESP32-S31 legacy advertising admission boundary

The open radio role, runtime and Controller core that this page compares
with the vendor are not in this repository; the passages that name them
describe an implementation outside this repository, which the memory
codecs, executor and portable pieces here follow.

This reference describes the hardware contract below the portable
`ADV_NONCONN_IND` encoder. It does not reproduce the vendor advertising
driver. The nonconnectable profile is restricted to LE 1M, standalone
always-awake operation, one non-connectable PDU on primary channels 37, 38 and
39, no receive window, no scan response and no coexistence policy.

## Evidence and role map

- ESP32-S31 Controller archive
  [`espressif/esp32s31-bt-lib@10c507788e9da0993709cf82e405c896561172d8`](https://github.com/espressif/esp32s31-bt-lib/tree/10c507788e9da0993709cf82e405c896561172d8),
  `libble_app.a` SHA-256
  `e61c5f8b0e558df8c520bcedd78dd8b930c2b61350b08c03430b774275385723`
  and `libbtdm_common.a` SHA-256
  `389561cead8a68444b46118fb606d5742813c88f8a37c43d22be271aaa4bdb41`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  whose unobfuscated names reach the pinned symbols through the reviewed
  lineage in [`chips/esp32s31/vendor/facts/names/`](../vendor/facts/names/).

Every behavioral claim below comes from a complete pinned instruction body.
Role names come from the lineage where it names the body; otherwise the call
graph below the named role (its sole caller, callees and call order)
establishes it. Old bodies supply no behavior or layout.

| Pinned body | Reviewed role |
| --- | --- |
| `r_sym_ble_GMKJqD73JoiGMGSQZS6e` | `r_ble_lll_adv_legacy_pri_chan_pdu_make`: build a legacy primary-channel PDU |
| `r_sym_ble_KRGYBIa2UPdfUO6rFP89` | `r_ble_lll_adv_legacy_pdu_make`: checked legacy-PDU builder |
| `r_sym_ble_b3ZDeCz8LKJWE1p1nl50` | `r_ble_lll_adv_pri_chan_txbuf_alloc_and_make`: allocate and fill the TX chain |
| `r_sym_ble_7lsXnox2LxrGG0FmY7qR` | `r_ble_lll_adv_reset_link_state`: reset the advertising link state |
| `r_sym_ble_ouZL9YSCqJEyaSmJ2xJF` | `r_ble_lll_adv_reset`: reset an advertising role |
| `r_sym_ble_BU65xm0Suu1ZxafP2o1R` | `r_ble_lll_adv_alloc_memory`: allocate the private advertising graph |
| `r_sym_ble_jDCJzglWI0uBeh2sNaOW` | receive-buffer allocation, the sole RX allocator below `r_ble_lll_adv_alloc_memory` |
| `r_sym_ble_0ByYfGTdjlqZTcq6wH2O` | advertising start, the sole caller of the allocation, reset and first-event bodies |
| `r_sym_ble_JqJr96pi4W7h47ZTV2Z8` | scheduler-item allocation for the started role |
| `r_sym_ble_5ZvxtGNXxB5HYDOl4nsn` | `r_ble_lll_adv_init`: initialize the first-event delay |
| `r_sym_ble_GlcyfUkkhUzGUt8un0d8` | `r_ble_lll_adv_sched_first_pri_event`: schedule the first primary event |
| `r_sym_ble_VJhDIFgEJhr4DAUSUJBU` | `r_ble_ll_adv_set_sched`: form one advertising scheduler window |
| `r_sym_ble_eNifqLwR78cnxeKb1y6t` | chain the remaining primary channels of one event |
| `r_sym_ble_bKWMq69wmtmvM3jpReNR` | `r_ble_lll_adv_sched_next_pri_event`: schedule the next primary event |
| `r_sym_ble_Mbq9uRHiK6RiYjkK9zIv` | recycle a completed scheduler item, installed by the item allocation |

The shared finished-list ingress and scheduler-list consumer are already
reviewed in `bluetooth-interrupt-runtime.md`. The focused binary-analysis
scope `ble-legacy-nonconnectable-admission` connects the exact packet, link-state and
first-event producers plus recycle to those two asynchronous roots. The wider
`ble-legacy-nonconnectable-advertising` scope retains broad role allocation,
reset, start and recurrence research.

## Nonconnectable event ownership

The portable Link Layer validates address kind, advertising data, non-empty
primary channel maps and interval bounds. It encodes an exact bounded
`ADV_NONCONN_IND` and retains generation, event and the complete ordered
channel plan through an affine lifecycle. The selected channels form one
backend event rather than separate executor-driven submissions. The S31
pre-admission owner installs that PDU in the common typed controller TX
allocation used by DTM and advertising. That allocation lives inside a pinned,
physically bounded graph. The graph
binds the common TX header to its packet, installs that header as the sole TX
head/tail, keeps the RX chain absent, retains one separately allocated common
scheduler context, binds up to three scheduler items back to this link state
and installs the first item as the link-state scheduler head. The semantic
channel plan stays at the portable boundary; only the private memory codec
lowers it to S31 frequency fields and private compressed successor links. The
owner supports lossless cancellation of both portable and SRAM owners. The
complete first-event path joins the descriptor graph
to common scheduler bookkeeping, an independently proven empty list, typed
`HEAD` publication, dynamic interrupt publication, the synchronous scheduler
event and `RUN`. Publication is the sole transition that moves the portable
owner to `InFlight`.

The public named archive proves that legacy primary allocation calls
`r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr`; its allocation prefix and PDU
placement are therefore shared with DTM. The DTM graph typestate itself is not
reused: advertising still has a different private link-state/scheduler graph
and recurrence policy.

The same archive's `r_ble_lll_adv_alloc_sch_items` establishes the separate
common scheduler-context link, the scheduler-item-to-link-state link and the
terminal first-item chain. `r_ble_lll_adv_start` independently stores that
item as link-state scheduler head before calling
`r_ble_lll_adv_sched_first_pri_event`. The pinned start body calls the graph
allocation, the link-state reset and the scheduler-item allocation in that
order, and the first-event body takes its item from the link-state scheduler
head at `+0x64`. These allocation-time
links are one private memory-codec operation; no compressed image or SRAM
field escapes to the controller or Link Layer.

The pinned `r_ble_lll_adv_reset_link_state` body
(`r_sym_ble_7lsXnox2LxrGG0FmY7qR`) establishes the next transition. The private
memory codec applies their restricted LE 1M, no-RX, no-CTE and no-privacy
projection from the prepared PDU and a signed dBm request. This includes the
terminal TX-header link, absent RX link, shared rounded-power conversion,
primary-advertising Access Address and CRC preset, public/direct-random address
branches and the reviewed standalone option byte. The option value is not a
guessed bit meaning: the reset body copies six bits of byte `+0x2b` of the
private options, which `r_priv_sdk_config_options_init` copies from the
`0x48`-byte default `sym_controller_jqcSm1kAtUzAMoyGlaKq`, whose byte there
is `3`. Reset creates
a separate non-publishable typestate; cancellation clears the packet and
rebuilds the allocation graph before returning it to the portable owner.

The pinned `r_ble_lll_adv_sched_first_pri_event` body closes the
first-event producer. `r_ble_lll_adv_init` stores a 2000-microsecond
first-event delay. The first event selects the channel, binds the item to the
link state at `+0x08` (to the separate channel-39 link state only when the
option at controller-options `+0x62` bit 2 enables separate channel-39 power;
the default disables it), clears item `+0x00` bit 22, writes the frequency
into `+0x18` bits 14:8, sets `+0x04` bit 31 and clears the rate nibble in
`+0x14` bits 31:28. It then samples the radio path and the scheduler time,
adds the delay (through an identity helper) and the scheduler lead that
`r_sym_sched_GEBmwfVsspx61ARDIjlz` returns, and lets `r_ble_ll_adv_set_sched`
place the window: the item starts at the sampled time plus the delay and ends
the lead plus the LE 1M airtime of the PDU later. If the radio observation is later than the start, it
shifts start and end together and preserves duration. Both positions pass
through the retained scheduler epoch into raw controller time. The priority
nibble of link-state `+0x60`, one for a legacy set, fills both nibbles of
item `+0x18` bits 7:0. The item then enters the list through
`r_sched_txn_delayIfOverlap` (`r_sym_sched_r2iinWC2SzzEDZskjGMQ`), which
displaces it past an overlap.

The first-event and `r_ble_lll_adv_sched_next_pri_event` bodies both end in
`r_sym_ble_eNifqLwR78cnxeKb1y6t`, which establishes that selected primary
channels are not resubmitted after an IRQ. Before the first `RUN`, it takes
every remaining selected channel, advances each follower start from the
previous item end, adds the same item duration and inserts the follower
directly after its predecessor. Unlike the first item, a legacy follower sets
item `+0x00` bit 22. With the default controller options, bit 1 of `+0x62`
(delay channels 38 and 39 past a conflict) is clear, so a follower is not
displaced. The completed-queue link at item `+0x54` is a different software
ownership link, not the execution chain. The next event recomputes the
rounded power into link-state `+0x61` and restarts from the configured
interval; that recurrence belongs to the portable Link Layer here.

The private SRAM codec therefore detaches the allocation-time scheduler head,
lowers the canonical selected channels 37, 38, 39 into 1--3 items that the
executor inserts in order,
selects the legacy LE 1M transmitter role, copies the reset link state's
rounded power, installs contiguous accepted raw windows and clears bookkeeping
for every active item. No link image, field mask, rounded-power image or
frequency integer crosses into the controller/LL layer. Before this mutation,
the radio role admitted the event against one fresh Controller-time sample and
the executor's list mirror: each channel reserves one channel spacing from its
anchor minus the preparation lead, and an overlap is rejected instead of
displaced. Rejection leaves the configured set unchanged. Like the vendor,
the codec clears item `+0x00` bit 22 on the first item and sets it on every
follower, so channels 38 and 39 may start as soon as the previous channel's
item ends, never later than their programmed anchors. The executor still
inserts each item by its own reserved window.

## Reviewed response-capable memory profile

The response-capable extension is derived from the pinned bodies in the role
map above: the link-state reset, the TX-chain allocation, the receive-buffer
allocation and `r_ble_ll_adv_set_sched`. Pinned bytes, rather than the
lineage names, own every layout and arithmetic fact below.

An `ADV_IND` event is a two-transmit-node graph, not the nonconnectable graph
with an optional receive flag. The primary builder always allocates the first
header and packet. Its response-capable branch allocates a second header and
packet, encodes the second PDU as `SCAN_RSP`, links the primary header to that
successor and installs the second header as TX tail. An empty scan-response
data body still retains the second node. The reset body copies the primary
TX header's successor into the compressed scan-response consumer.

The controller TX allocation is not a contiguous on-air advertising PDU.
The pinned TX-chain allocation passes packet `+0x12` as the data destination
and stores the builder's returned on-air payload length at `+0x11`. The
pinned legacy primary-channel builder copies only the advertising or
scan-response data for the undirected path, while its returned length includes
six additional address bytes. For `ADV_IND` it sets the ChSel header bit when
controller-options `+0x57` enables Channel Selection Algorithm #2. Hardware inserts `AdvA` from
the selected address source. The open legacy TX codec therefore preserves
the complete canonical PDU for CPU access, but omits `AdvA` from the controller
data bytes and retains the on-air length. This applies to `ADV_NONCONN_IND`,
`ADV_IND` and `SCAN_RSP`; the common DTM/data packet codec retains its complete
payload representation. Directed advertising is outside this projection.

The global RX path depends on the inputs and ordering of several producers.
The pinned start body allocates the role before resetting its link state. Its
allocator reaches the receive-buffer allocation, which clears the software RX
head, tail and reserve at link-state `+0x68`, `+0x70` and `+0x78` on entry.
Consequently advertising reset leaves the compressed private RX consumer
empty. The later global RX-link update selects global non-scanning class two
and snapshots the global head/tail into the software endpoints: its callee
`r_sym_memMgmt_fklZTFbPfOGtnTfZVgMq` stores only link-state `+0x68`, `+0x70`
and `+0x78` and does not write the compressed private consumer. The response-capable readiness predicate
therefore requires the scan-response consumer, the retained global RX
endpoints and an absent private RX consumer.

The fixed open pool publishes a completed packetless header as the initial
global cursor, with two writable packet nodes after it. The extraction owner
copies only packet-bearing nodes. The vendor keeps a global RX chain across
events and returns processed buffers through `r_ble_lll_append_rx_buffer`.
The fixed pool's hardware rotation and retirement are not qualified by its
host model or by scheduler-item completion alone.

When response capability is present, the scheduler-window body adds the
halfword at `+0x2c` of the public Controller configuration that
`r_sdkconfig_get_opts` returns to the LE 1M primary-PDU airtime. That field is
`esp_bt_ctrl_le_config_t::ble_ll_sched_adv_max_usecs`, 852 microseconds by
default (`BLE_LL_SCHED_ADV_MAX_USECS_N`). Its directed branch uses
`ble_ll_sched_direct_adv_max_usecs` at `+0x2a` instead. The open radio does not
derive the item length from this field: each channel reserves the channel
spacing the portable Link Layer plans. The receive allocator prepares the non-scanning receive
class and supplies the selected primary-channel count to vendor memory
management. The memory typestate lowers one scheduler item per selected
primary channel, as for non-connectable advertising; every item shares the
two-header TX chain and the global non-scanning RX chain. The integrating
firmware sizes that chain for one reception per channel plus the connection
indication, and the radio drains it when the event ends. Delivery of every
reception of a three-channel event is not yet qualified on hardware.

The portable Link Layer owns the semantic advertisement, scan-response data
and typed bounded `ADV_IND`/`SCAN_RSP` encodings. A later chip aggregate must
retain that affine portable event while translating it into a short-lived S31
memory input: two allocation-fit PDU borrows, the already selected own-address
behavior and the translated primary-channel plan. The memory crate has no
portable Link Layer dependency, does not retain that event and does not parse
or revalidate Bluetooth wire semantics. It owns only the private two-header TX
chain, the exact affine non-scanning RX pool, the common advertising reset
projection, global RX-list binding and the opaque scheduler duration.
The joined memory owner remains non-publishable and losslessly returns only the
graph and RX pool on cancellation or rejection. Scheduler admission, RX
dispatch, accepted `CONNECT_IND` transfer and multi-channel buffering are
separate later boundaries rather than implied capabilities of this memory
profile.

The complete common allocation and pre-publication producers also participate
in this graph. The pinned common scheduler-item allocator
`r_sym_memMgmt_DJqpkHU8oAe5YxcQ1aHk` zeroes the item, sets both allocation
bits (`+0x00` bits 21:20), clears byte `+0x18` and installs the module-default
projection at `+0x1c`. The advertising scheduler-item allocation then calls
`r_ble_lll_adv_coex_pti_init` (`r_sym_coexAdv_pNmEzY32xPoYRR8VuoXP`), which
writes four five-bit radio-request priorities at item `+0x24`. With another
radio on the antenna the vendor applies dynamic priority control. That body
fills the four lanes of a legacy set from two short default lane tables of
`coexAdv.c.o_1.o`, one for the raised lanes and one for the low lanes.
`r_sym_coexAdv_sLW7oGzvK2Nq64ivPWK2` then replaces lane zero for every event
with an entry of the legacy table `sym_coexAdv_H8ZFnQsVsYzrsw6Rv2Wu`, indexed
by the event's level. The same table holds one event period per interval band
and the 16-bit interval thresholds, in 625 us units, that bound the bands. The
lane values and thresholds are to be measured on the stand: dump item `+0x24`
from a running vendor image under shared-radio load, and infer the thresholds
from which events are raised. The decision rule, which raises every Nth event
with N chosen by the interval band (`r_sym_coexAdv_VG7v...`,
`r_sym_coexAdv_A5DX...`), is recorded in
[LE coexistence priorities](bluetooth/le-coexistence-priorities.md).
Alone on the antenna,
every lane requests 15; this equal standalone policy is a product choice.

The pinned global RX-link update `r_sym_memMgmt_4xg1uFByb1OqQ8sR82yB` selects
non-scanning class two in link-state `+0x20` bits 30:28, matching selector two
of the RX publication transaction; its remaining state lives in the separate
software RX-link object at link-state `+0x7c`.
The reset body alone does not install this later memory-manager effect.
The RX pointer publication also applies the separate common RX-list reset
suffix described in [the RX-list contract](bluetooth-passive-scanning.md):
clear the reviewed current-pointer control field, then preserve a fresh
observation through the following write-back.

## Scannable and directed sets

The pinned bodies select the graph from the advertising properties at
`advsm+0x0c` (bit 0 connectable, bit 1 scannable, bit 2 directed, bit 3 high
duty cycle, bit 4 legacy). The TX-chain allocation
`r_sym_ble_b3ZDeCz8LKJWE1p1nl50` builds the second, `SCAN_RSP` node exactly
when bit 1 is set, so `ADV_SCAN_IND` uses the same two-node graph as
`ADV_IND`. The receive allocation `r_sym_ble_jDCJzglWI0uBeh2sNaOW` enables
receive-buffer insertion whenever bit 0 or bit 1 is set, and the scheduler
window body `r_sym_ble_VJhDIFgEJhr4DAUSUJBU` adds the response-capable window
under the same condition. The advertising reset
`r_sym_ble_7lsXnox2LxrGG0FmY7qR` differs only for a directed set, whose filter
policy field at link-state `+0x2c` bits 25:24 it clears.

A directed set is therefore connectable but not scannable: it receives on the
non-scanning chain with a single TX node and no scan-response consumer. The
legacy primary-channel builder `r_sym_ble_GMKJqD73JoiGMGSQZS6e` writes the
`ADV_DIRECT_IND` header with the ChSel bit and RxAdd for a random target,
copies TargetA as the controller data and returns a 12-octet payload; the
hardware inserts AdvA as for the undirected PDUs. The S31 memory profile
takes the scan response as optional and, without one, leaves the primary
header without successor, makes it the TX tail and keeps the compressed
scan-response consumer empty. The radio contract names what a set listens
for: nothing, scan requests answered by a scan response, or receptions only.

The removed Controller core accepted a connection indication from a scannable set
never and from a directed set only when InitA equals the target. High duty
cycle directed events follow each other 3.75 ms apart without the
advertising delay and end after 1.28 s with LE Connection Complete status
Advertising Timeout.

## Minimum production admission contract

Legacy connection channel selection uses both transmitted ChSel fields, as
specified by Core 5.4, Vol 6, Part B, sections 2.3.3.1 and 4.5.8. An initiator
supporting algorithm two may send ChSel one to an advertiser that sent ChSel
zero; the resulting connection uses algorithm one. The portable admission
retains the decoded request and separately initializes the negotiated selector.
The pinned `r_ble_ll_conn_slave_start` (`r_sym_ble_SVIOVPSLPVyCJcQuLud7`)
likewise tests the local algorithm-two option at controller-options `+0x57`
before interpreting the legacy request bit. Its
disabled branch initializes algorithm-one hopping and continues connection
setup rather than rejecting the request.

One advertising transmission may enter production only when current-artifact
evidence closes the following connected edges:

1. **Packet producer:** one owned packet/header image contains the encoded PDU,
   declared length, advertising access address `0x8e89bed6`, CRC initialization
   `0x555555`, selected primary-channel frequency and matching whitening seed.
2. **Graph binding:** the link-state and scheduler item point to exactly that
   packet storage, and every hardware-followed pointer has a bounded typed
   encoding, alignment rule and lifetime.
3. **Publication:** a single affine operation orders all SRAM writes before
   publishing one common-scheduler list head and issuing `RUN`. CPU mutation is
   impossible while the owner is hardware-visible.
4. **Terminal observation:** the interrupt/finished-list path identifies the
   same item, fences hardware writes, unlinks it from both hardware and
   software lists and returns CPU ownership exactly once.
5. **Result and recurrence:** every active item must have a non-sentinel
   completion before the exact scheduled event advances its portable identity
   once. Per-item values remain diagnostic rather than a claim of successful
   on-air transmission. The first slice needs no RX or scan-response handling.

Unknown private fields do not block the first driver merely because they lack
vendor names. They do block admission when their value participates in packet
selection, timing, ownership, launch or completion. A reviewed whole producer
image with controlled inputs is sufficient; guessing individual bit names is
not required.

## Completion and recurrence

Scheduler item `+0x38` uses an all-ones in-flight sentinel. Both zero and
nonzero terminal values consume the primary-channel item; nonzero values
retain diagnostics rather than representing a retryable pre-publication
failure. Every active item must be terminal before the exact affine LL event
advances. Terminal diagnostics alone do not prove on-air success.

The selected 1–3 primary channels form one advertising event of one item per
channel. Recurrence, the 0–10 ms advertising delay and live timing belong to
the Link Layer core that plans each event; every event passes the same
admission, publication and completion path. Response-capable RX and accepted
`CONNECT_IND` transfer have their own memory and LL contracts.

The response-capable path applies the common
[`r_btdm_sched_calc_seq_time` projection](bluetooth-direct-test-mode.md) after
fresh sequence authorization: sequencer start is the accepted raw start plus
the reservation's configured raw lead, and duration is the wrapping accepted
end minus start. These hardware timing inputs are distinct from the scheduler
window endpoints. The memory codec writes both before bookkeeping and RUN;
rejection retains the unchanged CPU owner. Nonzero completion diagnostics retain
the complete opaque item value.
