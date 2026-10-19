# ESP32-S31 BLE peripheral-connection hardware boundary

The open radio role, runtime and Controller core that this page compares
with the vendor are not in this repository; the passages that name them
describe an implementation outside this repository, which the memory
codecs, executor and portable pieces here follow.

This reference describes reviewed hardware-facing facts consumed by the Rust
driver. Vendor Controller architecture does not define the open ownership model.
Portable Link Layer policy remains in `portable/bluetooth/le/ll`; this document
identifies the S31 controller-SRAM fields and event edges that a chip backend
must lower.

## Authenticated connection contracts

The connection functions of the pinned
[`esp32s31-bt-lib@10c5077`](https://github.com/espressif/esp32s31-bt-lib/tree/10c507788e9da0993709cf82e405c896561172d8)
`libble_app.a` are identified below by their symbol names; role names come
from the reviewed lineage in
[`knowledge/esp32s31/vendor/facts/names/`](../../../knowledge/esp32s31/vendor/facts/names/)
or, where it names no body, from the pinned call graph.

`r_sym_ble_bgOSnaHsEjrTC0mkupqH`, the lineage's
`r_ble_lll_conn_reset_link_state`, proves two direct semantic transfers from the
connection state into the private `0x84`-byte link state:

| Semantic value | Connection state | Link state | Reviewed transform |
| --- | ---: | ---: | --- |
| CRC initialization | `+0x74` | `+0x2c` | low 24 bits are copied; the high byte remains owned by other link-state flags |
| Access Address | `+0x70` | `+0x38` | the complete 32-bit value is copied |

The named same-chip connection-input path independently shows that these
connection-state values originate from the Access Address and CRCInit octets
of `CONNECT_IND`. The Rust memory boundary therefore accepts their wire-order
octets and installs only those two reviewed fields. No integer link-state image
is accepted from the portable LL or exposed back to it.

The rest of `ble_lll_conn_reset_link_state` also initializes packet-length,
power, CTE, PHY-policy and private configuration fields. Those writes are not
yet assigned a source-owned connection-event contract and are deliberately not
part of the Rust identity transition.

The separate `ble-connectable-advertising-response` scope closes the causal
path into that transition. Current `ble_lll_adv_rx_process` reads the received
packet owner, passes its PDU at the reviewed advertising-packet boundary and
converts the descriptor's captured time word at packet-prefix offset `+0x10`
with
`ble_phy_get_actual_tx_time`. That helper first converts controller ticks to
microseconds, then subtracts two PHY-mode-indexed calibration terms. The
result is an on-air packet-start time, not a packet-end time and not an
ordinary observation of controller `now()`.

The controller-memory codec copies that word into an opaque
`LePacketCapturedTime` beside the received PDU and RSSI. It exposes
no field mask or scheduler-time claim. The published task service performs
the only permitted conversion: the opaque value enters the retained S31
scheduler epoch without re-anchoring it and then the initialized PHY
calibration. The result is a single-use `Le1MPacketStartTiming`; no
raw tick or scheduler image escapes that operation.

The calibration is not implicit zero state. Current `ble_phy_module_init`
copies three separately owned tables into the BLE PHY environment before the
normalizer can use them: the 40-channel frequency mapping, PHY-mode packet
prefix airtime and the receive address-capture delay. The channel mapping and
packet-prefix airtime are derived in Rust from the LE channel/PHY definitions;
only the S31 receive-capture delay remains a reviewed chip fact. The memory
owner keeps the resulting tables private and exposes a value-only LE 1M
normalization operation, so neither an extracted table nor its positional
indices cross into the Link Layer.

Current `ble_ll_adv_rx_pkt_in` routes PDU type 5 to
`ble_ll_adv_conn_req_rxd`; after address/filter admission,
`ble_ll_conn_peripheral_start` parses the request and reaches
`ble_ll_conn_created`. For a legacy primary-channel `CONNECT_IND`, connection
creation derives the first anchor as:

```text
normalized packet start
  + CONNECT_IND on-air duration for the received PHY
  + WinOffset * 1.25 ms
  + 1.25 ms
```

The first receive-window width is `WinSize * 1.25 ms`. This proves that the
portable LL may own PDU validation, address admission and transmit-window
arithmetic, while the S31 backend must supply a typed, PHY-calibrated packet
time. Exposing the raw descriptor time word or substituting a later live clock
sample would move an unresolved hardware transform into protocol code.

That boundary is implemented. Portable LL publishes the protocol-derived
LE 1M `CONNECT_IND` airtime and relative WinOffset/WinSize positions. The S31
connection runtime consumes the single-use packet-start value, adds the packet
airtime and relative positions with wrapping scheduler semantics, and retains
the resulting absolute first window beside the still-unsubmitted connection
event and identity-prepared SRAM graph. Cancellation returns both the pristine
graph and event counter zero; there is no `now()` input on this path.

The lineage identifies `r_sym_ble_tPr7egUaNHmqfcieCA5O` as
`r_ble_lll_conn_slave_new`, the peripheral's first event. Its pinned body
proves that the first event additionally depends on all of the
following:

- a live controller-time observation used with the connection interval to
  derive the first anchor;
- a separate prepared start/end scheduler window;
- the selected data channel lowered to radio frequency and PHY/rate fields;
- scheduler insertion, conflict handling and a role-specific completion
  callback;
- post-insertion retry/reschedule policy before the device is reported as
  connected.

The same body closes a useful descriptor subset without requiring names for
the vendor's private aggregate types. The Rust memory codec consumes only
semantic values and performs these positional transforms privately:

| Private object field | Source-owned input | Reviewed first-event behavior |
| --- | --- | --- |
| link state `+0x00` | owned empty TX sentinel | stores the compressed endpoint, 251-octet S31 capability and the two transmit-path ready states |
| link state `+0x61` | signed default TX power | index of the highest BTBB provider power level not above the request, beside the priority byte `+0x60` (pinned `r_ble_lll_conn_reset_link_state`, `r_sym_ble_bgOSnaHsEjrTC0mkupqH`); a request below the lowest level is refused |
| link state `+0x08` | owned initialized RX pool | stores the compressed packetless predecessor and initial unconsumed receive sentinel |
| link state `+0x0c` | S31 baseline control policy | installs the duplicated value 2 and makes that policy active |
| link state `+0x18` | absolute connection creation time, then hardware valid-RX time | positional epoch conversion to controller ticks; preserved between events and read after unlink |
| link state `+0x14`, `+0x1c`, `+0x20`, `+0x30` | new unencrypted connection | clears packet history/control state and installs the recovered initial sequence profile |
| link state `+0x2c` | CRCInit | preserves the low 24-bit CRC seed and marks that context ready |
| link state `+0x50` | powered epoch's opaque global workspace link plus S31 common-radio policy | retains the default value 3, stores the compressed `workspace + 8` endpoint, clears the separate four-bit mode and marks the direction-finding configuration ready |
| link state halfword `+0x56` | disabled-CTE ordinary-role policy | preserves the reviewed low flags, clears the unsupported mode region and installs the disabled baseline |
| link state `+0x60` | S31 first-event conflict policy | starts at 13; later conflict handling increases it and saturates at 15 |
| scheduler item `+0x04` | ready state | sets the reviewed context-ready flag |
| scheduler item `+0x14` | LE 1M plus rounded TX power | selects the LE 1M rate lanes and copies the rounded-power projection |
| scheduler item `+0x18` | data-channel index plus bounded priority | maps data channels 0--36 to the S31 frequency image and copies the four-bit priority into both lanes |
| scheduler item `+0x2c`, `+0x2e` | first transmit-window width plus a symmetric timing guard | stores the short receive-wait duration and its fixed mode; the descriptor image remains private to the memory codec |
| scheduler item `+0x38` | new event | clears the initial status |
| scheduler item `+0x44`, `+0x48` | resolved common-scheduler window | stores the accepted start and end only after overlap resolution |
| scheduler item `+0x4c` | new event | clears the reviewed low bookkeeping byte |

The common sequence projection also applies to peripheral events. Current
`libbtdm_common.a` member `19.o`, `r_sym_bt_zpuzq1MeZSgFAehUuR9n`, is
byte-identical to named `r_btdm_sched_calc_seq_time` (146 bytes). It writes the
accepted start plus the converted sequence lead to item `+0x0c`, and accepted
end minus accepted start to `+0x10`. The software bounds at `+0x44/+0x48` alone
do not schedule a hardware receive event. Both first and recurring Rust paths
pass the reservation's actual timing-policy lead after sequence authorization;
the memory codec installs those hardware inputs without resampling time.

The connection allocator also retains the common allocation bits installed by
the common scheduler-item allocator `r_sym_memMgmt_DJqpkHU8oAe5YxcQ1aHk`. The
pinned connection allocator `r_sym_ble_fhQDfdlA6MRu4nAdDQN0` preserves them
while linking the item, then applies the connection-specific mask to the
common flags at `+0x1c`. Its call to `r_ble_lll_conn_coex_pti_init`
(`r_sym_coexConn_BqYvJSQvD3IUJJyL5jD8`) initializes the two five-bit
radio-request priorities at `+0x24`; advertising has a different, four-lane
producer. With another radio on the antenna the vendor applies dynamic
priority control: `coexConn.c.o_1.o` `r_sym_coexConn_BqYv...` installs equal
base lanes from a short default lane table, and `r_sym_coexConn_sQLo...`
replaces lane zero for every event with an entry of the table
`sym_coexConn_1jwNCSpbwgr6k9BEu0Eg`, indexed by the event's level. That table
holds per-level lane values, the counts used by the opening and protection
rules, and pairs of an allowed missed-event count and an interval threshold.
The lane values and thresholds are to be measured on the stand: dump item
`+0x24` from a running vendor image under shared-radio load, and infer the
thresholds from which events are raised. The decision rules (the first events
of a connection, more missed receptions than the interval band allows, and a
local control procedure raise the level; `r_sym_coexConn_zaMn...`) are
recorded in
[LE coexistence priorities](../../../knowledge/esp32s31/bluetooth/le-coexistence-priorities.md).
`r_sym_coexConn_H75Y...` protects the connection for a number of 625 us slots
from the same table, converted to units of 256 us in link-state `+0x30` bits
19:14 with bits 21:20 set; that count is to be measured on the stand by
reading link-state `+0x30` in a running vendor image. Alone on the antenna both
lanes request 15 and no protection is set; this equal standalone policy is a
product choice. Recycle and recurring cancellation retain these allocation
fields while replacing event timing.

The fixed open connection pool starts with a completed, packetless predecessor
followed by two packet-bearing nodes. Both the connection's private receive
cursor and the selector-two publication refer to that predecessor. The
controller advances to its successor before receiving; publishing the first
packet-bearing node would skip it and produce a completion-chain gap.
After connection recycle, the last completed packet node remains the hardware
current cursor. A third physical packet node lets the pool publish two writable
successors without rearming that current allocation. The initial event links
only its two-packet prefix; the spare is initially unreachable. This bounded
pool is an open ownership choice, not proof of equivalence to the vendor's
persistent global buffer rotation; the shared hardware cursor contract is
described in [the RX-list reference](bluetooth-passive-scanning.md).

The priority is not an application-supplied integer. The pinned
`r_ble_lll_conn_reset_link_state` stores byte `+0x27` of the private options
plus nine, or 15 when that byte exceeds five, as link-state priority `+0x60`;
the default byte is 4, so the first event carries priority 13. It copies byte
`+0x2b`, whose default is 3, into the common-radio policy. The first-event and
recurring bodies both write that priority nibble to item `+0x18` bits 3:0 and
a fixed 7 to bits 7:4; the open codec does the same. These scalars are reviewed chip policy inside the backend.
The channel-frequency mapping is the ordinary LE data-channel ordering around
the three primary advertising-channel positions; the portable LL still sees
only a validated data-channel index. Likewise, signed dBm, interval and a
non-empty wrapping window are the only dynamic inputs visible above the memory
crate. Masks, shifts, rounded-power values and SRAM offsets do not leave that
codec.

The minimum connection-event duration depends on the connection PHY.
`r_ble_ll_conn_get_min_dura_required` (`r_sym_ble_IXD9YD54AdqppMHisVZw`) and
the move body both index the table `sym_ble_Qw8LKJo0HxALvv5KCN4X` by that PHY.
It holds four durations in microseconds, LE 1M first, and its entries differ
from those of the named archive's `g_ble_ll_conn_evt_dura_ro`. The durations
are to be measured on the stand: for LE 1M, scheduler item end minus start of
a connection event; the other PHYs need 2M and Coded connections. The older
archive's values are not needed. The open radio reserves the LE 1M value.

The pinned `ble_ll_conn_created` (`r_sym_ble_62UX9ux9YcuUoIf4gPAp`) shows
that the first scheduler reservation does not end at the upper edge of the
transmit window: its end is the anchor plus `WinSize * 1.25 ms` plus the
table's duration for the connection PHY plus one. For LE 1M it retains the
table's LE 1M duration as further event time
and a one-unit boundary guard. The source-owned backend preserves that
complete reservation and begins it before the receive anchor by the common
preparation lead plus the open NimBLE 16-microsecond uncertainty guard and one
boundary unit. The portable LL still owns only WinOffset and WinSize.

This also agrees with the architectural split in
[public Espressif NimBLE](https://github.com/espressif/esp-nimble/blob/916be244a9c646bc16fd65507478cf3fe717d8ed/nimble/controller/src/ble_ll_conn.c#L2868-L2873):
its first peripheral event separately retains `periph_cur_tx_win_usecs` and a
connection-event deadline based on `BLE_LL_CONN_INIT_SLOTS`. The open source is
used to identify the two scheduling concepts, not to import its NPL scheduler
or make the vendor's connection aggregate an ABI.

The complete `ble_lll_conn_peripheral_new` body derives its receive-wait value
as `WinSize * 1.25 ms + 2 * timing_guard + 61 us`. Every valid legacy
transmit window fits its short descriptor form, whose encoding is private
to `PeripheralConnectionReceiveWait`. No upper layer accepts the
duration/configuration word.

The event remains deliberately CPU-owned. The direction-finding bodies of the
earlier `7f20740` archive and the named same-chip archive, not yet
re-established on the pinned body, show that ordinary advertising, sync and
connection link states all retain one controller-global `0x20`-byte
environment even when IQ sampling is disabled. The open driver claims and
initializes that separate static workspace before MMIO, publishes its disabled
descriptor through CTE buffer zero, clears software ownership through generated
PAC accessors and retains the joined SRAM/HAL owner for the complete powered
epoch. It does not reproduce the vendor allocator or make the workspace
connection-private. A distinct memory-layer transition consumes the
resulting opaque environment link, privately installs the `workspace + 8`
configuration endpoint and adjacent disabled-CTE policy, and returns a new
affine graph state. The radio role admitted each connection event against one
fresh Controller-time sample and wrote the requested window into the private
descriptor; the executor rejects an overlapping window instead of displacing
it, so the descriptor always carries the window the Link Layer core planned.

The allocation suffix of the pinned `r_ble_lll_conn_slave_new` behaves as
follows. The connection link state's selected
scheduler head is the item at the private free-list head. Allocation reads that
item's compressed predecessor, advances the private head to the predecessor,
detaches the selected item and passes only that item to the common scheduler;
it does not publish the complete two-item private chain. The memory codec
models that transition explicitly and hands only the detached item to the
executor. No compressed pointer or list word leaves the memory crate.

These dependencies explain why Access Address plus CRCInit is not a runnable
event image. A connection receives through its own class-zero chain, whose
consumer is link-state `+0x08`, as `r_ble_lll_conn_use_rxbuf_from_link_state`
selects; the connection pool instance owns that chain. Each event stays with
the executor from list-zero insertion through `RUN`, the fenced finished-list
capture and the completion walk. The completion walk returns the item; the role
then copied every contiguous completed RX PDU, reported the captured anchor and
the peer's acknowledgement, and preserved the live connection link state for
the next event.

The lineage identifies `r_sym_ble_mLYtT1I4BQmunJL36NqV` as
`r_ble_lll_conn_update_link_state`, which reads and writes this word. Together
with the default and custom aborted-opcode writers, its complete body identifies
the word at `BTMAC_BLE_PHY_INIT + 0x4ac` with a narrower reviewed
contract. Bit 0 selects the custom aborted-opcode path. Link-state refresh
independently replaces bit 1, clearing it when the second private connection flag is set,
independently of the first, and setting it otherwise. The register is published as
`CONNECTION_ABORT_CONTROL` with field accessors.

The default opcode tables contain `0x02`, the plaintext `LL_TERMINATE_IND`
opcode. Software CCM ciphertext can have that same first byte. The connection
RX publication therefore clears `LINK_STATE_CONTROL` before publishing the
head, leaving opcode interpretation to the authenticated software LL decoder.
The vendor reapplies this policy on every connection publication and restores
the initialization policy for advertising and scanning publication. The radio
runtime publishes both global chains once per powered epoch, before the first
RUN: the scanning chain with the initialization policy, then the non-scanning
chain with the software-connection policy, which stays in force for every
later event. Whether the policy affects non-connection receptions, and whether
shared-PHY maintenance clears it so that it must be republished on resume,
are open hardware questions. The [HAL transaction](../../../chips/esp32s31/hal/src/bluetooth.rs)
owns that sequencing; cold BLE PHY initialization retains its reviewed vendor
images. Neither raw register images nor private vendor flags cross into the
portable Link Layer.

## Completion recycle and recurring-event facts

The pinned call graph identifies the complete connection-event suffix:

- `r_sym_ble_eDUBAKBNjv0YRQI9tlHA`, the recycle callback the connection
  allocator installs, is `ble_lll_conn_recycle_sch_item`;
- `r_sym_ble_AFktzwzXYWsJzbJozyXH`, reached from it and from
  `r_ble_lll_conn_event_delete_and_reschedule`, is
  `ble_lll_conn_sched_next_anchor`;
- `r_sym_ble_rsCCyH2B22gdYkN4LOOJ`, its callee that refreshes the link state
  and inserts through `r_sched_txn_insertOne`, is
  `ble_lll_conn_reschedule_event`.

The scheduler-item status is not a success/error boolean. Zero takes one quiet
bookkeeping path. A nonzero value adds diagnostic accounting and then exposes
independent flag branches; both zero and nonzero items are returned to the
private free list, and either may later continue or destroy the connection
according to separate connection state. The Rust status must therefore remain
`Zero` versus opaque `NonZero`, and LL event completion must not be selected by
that distinction alone.

The recycle body first pushes the completed item onto the connection-private
free list: it preserves the allocation prefix, installs the retained private
head as predecessor, then makes the completed item the new private head. The
reschedule body pops that same item, clears its status and rebuilds its event
fields; failed insertion pushes it back. This confirms that completed common
list removal returns an event-local scheduler item while preserving the live
link state.

Recurring events use a different profile from the first event:

- baseline priority is 8, with conflict escalation saturating at 15;
- the receive wait includes accumulated window widening and controller-owned
  guard terms and selects a short or long descriptor form;
- the proposed anchor advances from the prior nominal anchor, never from a
  fresh `now()` sample;
- missed intervals advance the proposed event counter by `skipped + 1`, while
  admission retries remain provisional until one candidate commits.

The pinned bodies make the software-widening arithmetic exact rather than
heuristic. The move body `r_sym_ble_JqbtnypGd2wyQym9wmK4`
(`ble_lll_conn_move_to_next_event`, reached from the next-anchor body) calls
the widening helper `r_sym_ble_W6sXwmh4TbZnhptZU0wo` with the proposed anchor, the last actual
anchor and the two SCA indexes. That helper converts the positive elapsed time
to microseconds, truncates it to whole milliseconds, multiplies it by the sum
of the two worst-case PPM table entries, and divides by 1,000 again. Both
divisions are unsigned truncation, so the controller uses floor rather than a
conservative ceiling. The move body then adds byte `+0x26` of the public
Controller configuration that `r_sdkconfig_get_opts` returns,
`esp_bt_ctrl_le_config_t::ble_ll_jitter_usecs`, 16 microseconds by default
(`BLE_LL_JITTER_USECS_N`).

The software branch of `ble_lll_conn_reschedule_event` subtracts the
preparation lead, byte `+0x22` of the private options, the current widening
and one CPU-time boundary tick from the proposed anchor. The private options
are the `0x48`-byte default `sym_controller_jqcSm1kAtUzAMoyGlaKq` copied by
`r_priv_sdk_config_options_init`; its byte at `+0x22` is 10 and S31 CPU-time conversion is identity, so that boundary tick
is one microsecond. Its receive-wait duration is the same 10-microsecond fixed
guard plus accumulated anchor uncertainty, twice the current widening and
`os_cputime_ticks_to_usecs(2)`, which is two microseconds on S31. This is
deliberately distinct from the 61-microsecond final allowance used by
`ble_lll_conn_slave_new` for the initial event. The recurrence must not carry
that initial-only 61-microsecond term forward. The move body obtains the
minimum event duration before adding accumulated uncertainty and current
widening to its proposed end; it is the same per-PHY minimum the first-event
reservation uses (its LE 1M value is to be measured on the stand). Thus the
recurring window ends at
`proposed - preparation + minimum + accumulated + widening`,
without either the start-only 10-microsecond guard or the initial-only
61-microsecond allowance.

The source-owned recurrence admits software clock widening with no detached
accumulated vendor guard. Before an actual packet establishes the anchor, its
typed phase retains the complete `CONNECT_IND` WinSize in both the receive wait
and reservation end, as required by Core Vol 6, Part B, 4.5.5. Each unanswered
window advances by one interval without replacing the widening reference. A
normalized packet replaces the uncertain window with an actual anchor. Other
nonzero vendor accumulation and automatic widening remain unavailable.

### Event span beyond the reservation

The reservation's per-PHY minimum is what the scheduler item holds;
they do not bound the exchange. The pinned connection-event setup
`r_sym_ble_rsCCyH2B22gdYkN4LOOJ` writes the item end from the reservation but
installs the link-state event span from the connection interval: the whole
interval when it is at most 7,499 microseconds, otherwise the interval less
2,000 microseconds plus the peripheral's current window widening. The
widening starts as the Controller configuration's `ble_ll_jitter_usecs` in
`ble_ll_conn_created` and the move body replaces it every event. A third
branch uses the maximum packet times plus 2,150 microseconds after an event
whose maximum-length exchange did not fit the interval less 2,150
microseconds; on LE 1M two maximum packets take 4,240 microseconds, so that
branch cannot occur above 7,499 microseconds.

The radio contract therefore carries the connection interval with every
event and the widening with every recurring event. The S31 backend derives the
span from them, with the first event's 16-microsecond guard as its widening,
and reserves only the minimum duration.

### Distinct event-span and captured-anchor fields

The memory codec writes `LINK_STATE_EVENT_SPAN` in the link-state object and
reads `SCHEDULER_ITEM_CAPTURED_ANCHOR` in the scheduler item after observing
capture-available status. Both offsets are `+0x34`, but their base objects are
different. Equal offsets do not establish an input/output alias. Base-pointer
provenance in authenticated first-event, reschedule and recycle bodies is
required before changing this distinction; the codec alone is not hardware
qualification evidence.

The chip timing path consumes the whole recycled owner into a distinct
packet-start-normalized typestate. It projects the connection capture through
the retained scheduler epoch and the existing LE 1M PHY calibration without a
fresh time sample or epoch reanchor. An unavailable epoch returns the complete
unchanged recycled owner. The transition retains status, RX batch, memory and
the in-flight portable LL event without interpreting or advancing any of them.

The first completion with a capture corrects the recurring phase from the
packet-start. On the software window-widening path, the actual start replaces
the committed anchor, the proposed anchor moves by the same delta, the
fractional residual and accumulated guard reset, and the actual start becomes
the widening reference. The separately identified mode flag selects automatic
window widening. The initial source-owned driver may support only the software
path and fail closed if automatic widening is unexpectedly active; the raw
vendor flag must not enter portable LL.

Recurring admission must distinguish skipped protocol intervals from events
that executed in hardware. Only an accepted proposal may commit LL time,
channel selection and event-counter advancement. Scheduler admission,
continuation/destroy classification, SN/NESN, retransmission, supervision and
ACL delivery retain separate ownership and evidence requirements.

## Connection control packet ownership

Connection RX uses the acceptance gate from current `conn_rx_process`, separately
from advertising RX. Rejected completed observations are counted and skipped
without hiding later accepted packets in the same bounded list. The connection pool retains its last completed RX descriptor and packet, and
rearms only the other two packet allocations as its writable successors. It does
not rewind the private hardware cursor or clear the adjacent controller word.
Extraction skips the retained current descriptor, preventing a previous packet
from being dispatched twice. The first event and every recurrence each have two
writable nodes; physical storage also retains the current node after connection
recycle. Software RX endpoints track this rotation. This is a
bounded counterpart of the vendor append/recycle path, not its dynamic allocator.

The live CPU-owned graph retains two TX headers and one bounded packet allocation.
Appending links a fully initialized successor after the current descriptor and
updates the software tail, following `conn_txbuf_insert_after`. Reclamation uses
the packet descriptor's completion, as in `get_txed_buffer`; scheduler completion
alone leaves the packet queued. When the completed packet is the tail, the
descriptor remains the hardware current cursor while its packet binding is
released. A later append reuses the other header. Recurrence does not reset
SN/NESN or the TX current path. Hardware publication remains the existing fenced
scheduler transition. The portable control responder owns LLCP payload bytes.

These contracts are a bounded implementation of the reviewed append and tail
reclamation paths. They do not establish equivalence with the vendor's dynamic
queue, priority insertion or encryption paths.

Peer termination and failed establishment use the completed, unlinked event owner.
The latter follows the six-event limit in Core Vol 6, Part B, 4.5.2; a peer
packet in the sixth event establishes the connection before that decision.
An accepted RX packet also establishes the connection when a capture is absent,
without manufacturing a timestamp; the timing phase retains the full WinSize
until an actual capture corrects it. Unestablished recurrence cannot skip an
intervening receive window. The
resulting internal disconnect reason is `0x3e`. Established-link supervision
uses the independent link-state receive timestamp and an elapsed-time deadline;
expiry retires with `0x08`. Retirement first
checks that the destination runtime is vacant and retains both allocation
identities. Rejection returns the unchanged completed owner. Success resets
the private graph and RX pool, cancels outstanding control payloads and restores
the original runtime allocation. Pending HCI response order remains retained
until publication; only then can the Controller accept another idle command.
This covers the bounded private-pool lifetime, not the vendor's full connection
timer, handle, ACL-credit or HCI disconnection lifecycle.


### Valid-RX time and established supervision

The memory input `PeripheralConnectionReceiveTime` is an absolute wrapping
controller timestamp, including zero at wrap. It is seeded from the borrowed
live first-admission sample. Initial scheduler admission still consumes the
original sample authority. Recurrence preserves this field; only the reclaimed
CPU owner exposes the hardware value after completion and unlink.

The reviewed current `r_ble_lll_conn_reset_link_state` body reads software
connection creation time and applies `r_sched_timer_convertTimeToTicks` before
writing link state `+0x18`. The completion recycle path applies
`r_sched_timer_convertTimeToUs` to that field and compares it with its saved
receive reference. A changed receive reference resets the supervision callout,
independently of the scheduler anchor-capture branch. The reviewed timeout
callback selects `0x3e` before establishment and `0x08` after establishment.
Anchor capture may establish the connection without refreshing valid-RX time;
the delivered RX batch is not an exhaustive record of empty or duplicate packets.

The composed Rust path derives the deadline from this hardware timestamp and
the negotiated supervision timeout, using the retained epoch without PHY
packet-start correction. Before sequence publication it compares a fresh
current sample with the deadline. Expiry cancels the unpublished recurring
reservation and retires the exact completed owner. A reservation beginning at
or beyond the deadline waits for expiry without publishing RUN. A RUN that
started earlier closes through the existing completion/unlink path before the
next decision. These source contracts do not qualify CRC-error or abrupt-loss
behavior, nor establish linked semantic equivalence with the vendor callout.
