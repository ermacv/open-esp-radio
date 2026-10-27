# ESP32-S31 legacy passive scanning contracts

The open radio role, runtime and Controller core that this page compares
with the vendor are not in this repository; the passages that name them
describe an implementation outside this repository, which the memory
codecs, executor and portable pieces here follow.

This reference covers legacy LE 1M primary-channel scanning, passive and
active, and the hardware observations needed for HCI LE Advertising Reports.
Portable LL policy, register transactions and private controller-SRAM
encoding have separate owners. Extended advertising and connection initiation
require distinct contracts.

## Pinned identity evidence

- ESP32-S31 Controller archive
  [`espressif/esp32s31-bt-lib@7a85006f35f6839ae7ea5adee16e68a757759196`](https://github.com/espressif/esp32s31-bt-lib/tree/7a85006f35f6839ae7ea5adee16e68a757759196),
  `libble_app.a` SHA-256
  `e9f0b462e5deaca7da82d91d0cbeaca54d9b3f97e25b033a8597eca72a06ebc6`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  whose unobfuscated names reach the pinned symbols through the reviewed
  lineage in [`chips/esp32s31/vendor/facts/names/`](../vendor/facts/names/).

All behavioral claims come from complete pinned bodies. A role name comes
from the lineage, or, where the lineage does not name the body, from its
position in the pinned call graph. The following identities bound the
passive-scanning slice.

| Pinned symbol | Role | Name evidence |
| --- | --- | --- |
| `r_ble_ll_scan_set_scan_params` | `r_ble_ll_scan_set_scan_params` | unobfuscated |
| `r_sym_ble_aAg5PASU4pZoEWJE6dZF` | `r_ble_ll_scan_set_enable` | call graph: sole scanner body below `r_ble_ll_hci_scan_set_enable` |
| `r_sym_ble_ZYt47DOz55jgTG2eS56W` | `r_ble_ll_scan_rx_pkt_in_on_legacy` | lineage |
| `r_sym_ble_r7iZNauw5tJsUKJQsMK0` | `r_ble_ll_scan_send_adv_report` | lineage |
| `r_sym_ble_d2T0aLFn5N34yLmrNhMQ` | `r_ble_lll_scan_alloc_rxbuf` | lineage |
| `r_sym_ble_aTsLUeVnygnblNndYYVB` | `r_ble_lll_scan_alloc_txbuf` | call graph: allocator callee reading the selected-PHY scan type |
| `r_sym_ble_IYA41eW9c6m45QdYupOB` | `r_ble_lll_scan_alloc_memory` | call graph: allocator called by the start body |
| `r_sym_ble_KkAldzIlkQuEkNQp1g6q` | `r_ble_lll_scan_reset_link_state` | lineage |
| `r_sym_ble_rEXt5T3VRIbMPSs1sbtt` | `r_ble_lll_scan_rx_process` | lineage |
| `r_sym_ble_CcSBujbD8nhmAY1yBT4T` | `r_ble_lll_scan_recycle_buffer` | call graph: item `+0x5c` callback installed by the allocator |
| `r_sym_ble_M0sTWGzdUqAUyXoK849F` | `r_ble_lll_scan_restart` | lineage |
| `r_sym_ble_jQKuzvSTa70k58IPazik` | `r_ble_lll_scan_chk_resume` | lineage |
| `r_sym_ble_6dUmBPDoEG5h9BPr5ciK` | `r_ble_lll_scan_recycle_sch_item` | lineage |
| `r_sym_ble_7mMdEaPRydkzNfvukINh` | `r_ble_lll_scan_stop` | lineage |
| `r_sym_ble_aQLjUHo25zeoHGKf9dby` | `r_ble_lll_scan_start` | lineage |
| `r_sym_ble_JywO1YDaUjiUQgSKMALX` | `r_ble_lll_scan_get_earliest_start_time` | lineage |
| `r_sym_ble_frMgZr376rodeRBQKDiO` | `r_ble_ll_resolv_list_find` | lineage |
| `r_sym_ble_5PtcKJoBdlB4XMIF3rZI` | `r_ble_ll_resolv_enabled` | lineage |
| `r_sym_ble_6lDKo33OEA8POtZnFQHv` | `r_ble_hw_resolv_list_search` | lineage |
| `r_sym_ble_vNVoB3u8sNalnntugADe` | `r_ble_lll_sync_set_scan_link_state` | lineage |
| `r_sym_memMgmt_HzBmGkHAh4mavF5QJM0o` | `r_ble_lll_get_rxed_buffer` | lineage |
| `r_sym_memMgmt_aK0VC1dzUQ8JFQTeTI66` | `r_ble_lll_set_rxbuf_default_value` | lineage |
| `r_sym_memMgmt_o8elWlcL1fxcPwWx5LoS` | `r_ble_lll_append_rx_buffer` | lineage |
| `r_sym_ble_Si8TRjTeEQJOviBabSuy` | `r_ble_lll_rxpdu_copy` | lineage |
| `r_sym_memMgmt_16QzTzT1uLme4dJZUFkO` | `r_ble_lll_mmgmt_rxbuf_cnt_get` | lineage |
| `r_sym_memMgmt_zscaM0ZVrKT1TdFnygSB` | `r_ble_lll_mmgmt_rxbuffer_disable_insert_check` | lineage |
| `r_sym_memMgmt_i12d5sbSaRnPvHCb07XD` | `r_ble_lll_mmgmt_sm_num_match` | lineage |
| `r_sym_memMgmt_fklZTFbPfOGtnTfZVgMq` | `r_ble_lll_update_global_rxlink_params` | call graph: stores the link-state reserve, head and tail |
| `r_sym_memMgmt_zJpaiu5wcM3sURVaC4Za` | `r_ble_lll_mmgmt_reset_rxlink` | lineage |
| `r_sym_memMgmt_6ULcUUTN7gWeV4tm1AFh` | `r_ble_lll_mmgmt_alloc_global_rxlink_mem` | lineage |
| `r_sym_memMgmt_AklYFPG5F14YLVXy8VHC` | `r_ble_lll_mmgmt_alloc_buffer_hdr` | lineage |
| `r_sym_memMgmt_vqXcstfse8788iyCxlJf` | `r_ble_lll_mmgmt_alloc_rx_buffer` | call graph: sized RX packet allocation below the count change |
| `r_sym_memMgmt_1mKWj9IUUHZ8L3iiGjwB` | `r_ble_lll_mmgmt_rxbuffer_cnt_change` | lineage |
| `r_sym_memMgmt_4xg1uFByb1OqQ8sR82yB` | `r_ble_lll_mmgmt_update_global_rxlink` | call graph: sole callee of the direct allocation |
| `r_sym_memMgmt_84gHKTNBZ7HZQvsRHKY5` | `r_ble_lll_mmgmt_rxbuf_direct_alloc` | lineage |

The independently named C61 body at
[`espressif/esp32c61-bt-lib@c800514c39a3e491bb13bb224987e109623d2cf2`](https://github.com/espressif/esp32c61-bt-lib/tree/c800514c39a3e491bb13bb224987e109623d2cf2)
corroborates the 102-byte `r_ble_lll_scan_chk_resume` identity.  It is not
register or ABI evidence for S31.

## Proven lower transaction

The pinned `r_ble_lll_scan_start` (`r_sym_ble_aQLjUHo25zeoHGKf9dby`)
establishes the outer start transaction without requiring the vendor's
software object graph:

1. select and allocate scanner memory through the scanner allocator
   `r_sym_ble_IYA41eW9c6m45QdYupOB`;
2. reset the hardware-consumed link state through
   `r_ble_lll_scan_reset_link_state` (`r_sym_ble_KkAldzIlkQuEkNQp1g6q`) and
   set bit 31 of link-state `+0x18`;
3. publish `1` to `BLE_SCAN_BACKOFF.BACKOFF_STATE_1` and `BACKOFF_STATE_0`, then either
   `1` or the controller option `scan_backoff_upperlimitmax` (options
   `+0x5c`, masked to nine bits) to `UPPER_LIMIT_MAX`;
4. wake the common RF owner and derive the first window start from the
   current time, the scheduler lead and the module's start delay;
5. call `r_ble_lll_scan_restart`, which places the channel in the first free
   scheduler item and inserts it, and retry only its scheduler-collision
   result `-2`, increasing the requested delay by 100 controller-time units on
   every retry.

The branch that chooses the `UPPER_LIMIT_MAX` value is reduced without guessing a
hardware field. The start body reads byte `+0x50` of the controller options,
`esp_bt_controller_config_t::ble.dis_scan_backoff`; the public
`ble_user_cfg.h` fixes `NIMBLE_DISABLE_SCAN_BACKOFF` to zero. A zero predicate
publishes the configured upper limit, while disabling the backoff publishes
`1`. The ESP-IDF S31 glue has no `scan_backoff_upperlimitmax` option; the
vendor reference on the stand read `0x20` from `UPPER_LIMIT_MAX` during an
active scan, so the S31 build's configuration carries 32 rather than the 256
default of other chips' `ble_user_cfg.h`. The three words therefore carry the Core Specification
scan-backoff state (Vol 6, Part B, 4.4.3.2): the maximum upper limit in
`UPPER_LIMIT_MAX` and the initial upper limit and backoff count of one in
`BACKOFF_STATE_0` and `BACKOFF_STATE_1`. The vendor's internal test command
`sym_ble_J4xWLLnJTN2yd1WPJrtH`, reached from
`api_internalTest_hci_procVsCmds`, reads `BACKOFF_STATE_1` and `BACKOFF_STATE_0` back as
two nine-bit values, so the hardware maintains that state; which word is the
count and which the limit is not established. The open Controller supports
only the default backoff policy, so the HAL owns one fixed
transaction with the default maximum and exposes no positional choice to
higher layers.

The already reviewed global-memory classifier is also applicable: scanner
scheduler kind two uses current/next RX selector one.  DTM's private graph is
not reusable here.  The scanner must own a normal current/next RX chain and
the hardware-to-CPU rotation, completion fence and backpressure that go with
it.

## Proven allocation boundary

The pinned common link-state allocator requests and clears exactly
`0x84` bytes. The restricted reset body writes only the bounded prefix recorded
below, while the same allocation retains the software RX head, tail and swap
reserve at `+0x68`, `+0x70` and `+0x78`. The open memory graph must therefore
reserve the complete `0x84`-byte allocation even though most trailing reset
words remain zero.

The same complete scanner allocator requests one `0x48`-byte scheduler context
and exactly three `0x60`-byte scheduler items. It chains each newly allocated
item before the prior item through the compressed hardware link at item
`+0x00`, installs the common context and link-state links at item `+0x04/+0x08`,
and retains the resulting head as a full pointer at link state `+0x64`. The
open graph reproduces those physical links but deliberately omits the vendor
callback pointers and intrusive software-list objects.

The pinned `r_ble_lll_scan_alloc_rxbuf` (`r_sym_ble_d2T0aLFn5N34yLmrNhMQ`)
does not allocate an opaque vendor scan object. It composes the common
receive-memory primitives around one already allocated link state:

1. it clears link-state words `+0x68`, `+0x70` and `+0x78` inline (the named
   archive's `r_ble_lll_rx_buffer_link_prepare(link_state, 1)` branch);
2. `r_ble_lll_mmgmt_rxbuffer_disable_insert_check(link_state, 1)`
   (`r_sym_memMgmt_zscaM0ZVrKT1TdFnygSB`) sets bits 1 and 3 of byte `+0x14` in
   the global RX-link object referenced at link-state `+0x7c`;
3. `r_ble_lll_mmgmt_rxbuf_direct_alloc` (`r_sym_memMgmt_84gHKTNBZ7HZQvsRHKY5`)
   receives the pointer stored at link-state `+0x64` and tail-calls the
   global RX-link update `r_sym_memMgmt_4xg1uFByb1OqQ8sR82yB`;
4. allocation succeeds only when `r_ble_lll_mmgmt_rxbuf_cnt_get(link_state)`
   (`r_sym_memMgmt_16QzTzT1uLme4dJZUFkO`) reports a nonzero count.

Reproducing a vendor heap or general-purpose mmgmt allocator is therefore not
a driver requirement. What remains to prove is the final selector-one RX-link
image produced by the global RX-link update, not the allocator's software
bookkeeping.

Complete current `r_ble_ll_scan_set_scan_params` validates the first HCI
payload octet as zero or one and stores it in the selected PHY configuration
byte at `+0x04`. Zero is the portable passive scan type, as independently
documented by the pinned
[`esp-nimble` controller source](https://github.com/espressif/esp-nimble/blob/916be244a9c646bc16fd65507478cf3fe717d8ed/nimble/controller/src/ble_ll_scan.c).
The scan type selects two things in the scanner allocator
`r_ble_lll_scan_alloc_memory`:

- Every scheduler item carries a kind in `+0x08` bits 23:20 beside its
  link-state pointer: `0xc` for a passive scanner and `0x4` for an active one.
  Hardware sends `SCAN_REQ` only from an active item; an active link state
  under a passive item receives advertising but never transmits.
- `r_ble_lll_scan_alloc_txbuf` returns at once for a passive scanner. For an
  active one it allocates one TX buffer, writes only the `SCAN_REQ` header
  (type 3, length 12) and stores the buffer in link state `+0x6c` and `+0x74`;
  the reset then publishes its compressed address in link state `+0x00`
  bits 19:0. Hardware fills ScanA from the own-address registers and AdvA from
  the received advertisement.

A passive scanner therefore needs an RX chain only. The open active scanner
applies the same kind and binding at reset. It still requests the passive
coexistence lanes below: the vendor's lanes for an active scanner, which also
transmits, have not been read.

## Proven receive graph and recycle transaction

The pinned common LLL and memory-manager bodies close the physical
shape of one scanner RX node without making the vendor allocator part of the
driver architecture.  Each node consists of a 24-byte header and a separately
linked packet allocation.  The header positions used by the pinned
bodies are:

| Header offset | Proven use |
| ---: | --- |
| `+0x00` low 20 bits | compressed successor-header link |
| `+0x04` low 20 bits | compressed packet-allocation link |
| `+0x0c` bit 31 | hardware-completion gate examined before a node can be returned to software; cleared before republishing the node |
| `+0x10` bit 0 | private list-rotation marker used by the common removal/append path |
| `+0x14` | full CPU pointer used while unlinking and reconnecting the chain |

The pinned `r_ble_lll_mmgmt_rxbuffer_cnt_change` requests a payload
capacity of `0xff` from `r_ble_lll_mmgmt_alloc_rx_buffer`.  The latter reserves
`0x1e + 0xff = 0x11d` bytes and writes the capacity plus two into packet bytes
`+0x05` and `+0x06`.  The packet positions needed by the first open role are:

| Packet offset | Proven use |
| ---: | --- |
| `+0x0c` low 24 bits | producer sentinel, reset to all ones before publication and required to change before accepted processing |
| `+0x0f` signed byte | receive-strength value copied into the LL receive metadata consumed by the legacy advertising-report path |
| `+0x18` low 16 bits | hardware-written receive cursor/epoch value; reset to all ones and checked both against the active memory-manager epoch and the bounded received extent |
| `+0x1c` | first on-air Link Layer PDU octet |
| `+0x1d` | Link Layer PDU payload-length octet |
| `+0x1e` | first Link Layer PDU payload octet |

The raw PDU location is not inferred from a diagnostic string.
`r_ble_lll_scan_copy_into_mbuf` passes packet `+0x1c` to the complete common
`r_ble_lll_rxpdu_copy` body and supplies exactly the `+0x1d` length plus the
two-byte advertising header.  The open driver can therefore parse the PDU
directly from its typed packet owner and does not need an `os_mbuf` clone.
Address kind is the standard TxAdd header bit and AdvA is carried by the first
six payload octets; those are portable LL semantics, not additional S31 SRAM
fields.

The complete recycle path is also closed:

1. `r_ble_lll_get_rxed_buffer` starts at link-state `+0x68`, rejects a header
   whose completion gate is still clear, validates the packet sentinels and
   active memory epoch, and returns only the corresponding header;
2. `r_ble_lll_scan_recycle_buffer` passes that header to
   `r_ble_lll_scan_rx_process` and never requires a vendor ULL allocation to
   read a legacy PDU;
3. after successful processing, `r_ble_lll_append_rx_buffer` restores both
   packet sentinels, clears the completion gate, reconnects the header after
   link-state `+0x70`, and updates the visible RX-link snapshot before another
   node is consumed.

The vendor's global RX-link object is software bookkeeping, not hardware
descriptor storage.  The pinned `r_ble_lll_update_global_rxlink` maps
scanner kind two to selector one.  Complete current
`r_ble_lll_update_global_rxlink_params` snapshots link-state reserve/head/tail
positions `+0x78/+0x68/+0x70` into that software object and performs stable
reads of selector one's current pointer; it does not publish a new MMIO
pointer.  The pinned `r_ble_lll_mmgmt_reset_rxlink` closes the cold
publication order for selectors one and two: after clearing the first
header's links and installing it as the software head/tail, it publishes that
header to `CurrentRx` and then publishes zero to `NextRx`. It subsequently
clears `RX_VALID` in the current-pointer register and writes back a second
fresh observation of that register, preserving any intervening hardware
change. Both selector-one and selector-two publication perform this suffix
through the HAL over the generated accessors; the broader meaning of that control bit
remains unassigned. The open driver owns a small fixed RX chain. The general
allocator, reference counts, callbacks and `os_mbuf` conversion are
deliberately excluded.

Focused binary inspection of the pinned
`r_ble_lll_mmgmt_sm_num_match` (`r_sym_memMgmt_i12d5sbSaRnPvHCb07XD`) closes the remaining vendor epoch check: when
the global RX-link object is enabled, it compares packet `+0x18` with that
object's halfword at `+0x18`; when the object is disabled it returns the
separate unavailable result. This is stale-allocation protection for the
vendor's dynamic manager, not another hardware descriptor input. The open
scanner has one statically bound affine graph and therefore replaces the
global manager epoch with stronger source-owned prerequisites: the completed
header, both changed packet sentinels, the exact finished-list/head/removal
proofs and the same non-reusable graph owner. No global vendor allocation
object or epoch field is reproduced.

## Restricted passive-1M reset profile

The pinned `r_ble_lll_scan_reset_link_state` body
(`r_sym_ble_KkAldzIlkQuEkNQp1g6q`) is `0x478` bytes; the named initial body
supplies only its role name. The current body changes private option and
`+0x18`/`+0x48` transforms; whole-profile equivalence is not established.
The open profile restricts operation to
passive scanning, public own-address type, accept-all filter policy, disabled
resolving and the selected standalone Controller options. Initiator,
active-scan, privacy and periodic-synchronization branches are excluded. The
open hardware-consumed link-state projection is finite:

- the low 20 bits at `+0x08` receive the compressed first RX-header address;
  the no-TX low-20-bit position at `+0x00` remains empty;
- the high-half reset profile at `+0x00`, the mode words at `+0x0c`, `+0x14`
  and `+0x18`, the allocation profile at `+0x30`, and the standalone option
  image at `+0x50` are all written before scheduling;
- the default transmit-power index is retained in byte `+0x61` even though
  this role does not transmit (the pinned `r_ble_lll_scan_reset_link_state`,
  `r_sym_ble_KkAldzIlkQuEkNQp1g6q`, does not write it into `+0x04`);
- `+0x2c` receives the advertising CRC preset `0x555555`, while `+0x38`
  receives the primary advertising access address `0x8e89bed6`;
- public own-address type and accept-all policy leave the privacy/filter
  selection at `+0x24` with an empty low-20-bit resolving-entry link;
- the reset stores the zero tick difference in `+0x34` and leaves the
  initiator-only address, timeout and connection fields untouched.

With scan filter policy 1 the same reset sets bit 25 of `+0x18` and writes
the Host's policy to bits 25:24 of `+0x2c`, beside the CRC preset; hardware
then reports only advertisers of the device table (see
[filter accept list](#filter-accept-list)).

The common sync helper is not an unresolved scanner MMIO operation. Its named
role is `r_ble_lll_sync_set_scan_link_state`. When periodic sync is disabled,
its complete branch performs two fresh-read updates that clear the sync-filter
and sync-link selections in
`BTMAC_BLE_PHY_INIT.DEVICE_TABLE_CONTROL`. It publishes the adjacent
`+0x478/+0x47c` values only when an enabled periodic-sync entry is selected.
The first passive-scanning slice keeps periodic sync disabled, so it needs
only the clear transition and no sync-entry storage.

The resolving accelerator is likewise not a blocker for this restricted
role. The current `r_ble_ll_resolv_list_find` wrapper and named same-chip role
call the complete `r_ble_hw_resolv_list_search` transaction: publish the list
and search arguments, select resolving configuration, clear the prior result,
start, poll the completion bit with a finite bound, and return either zero or
the result offset under the `0x2f000000` Controller-SRAM prefix. The common
vendor reset invokes it to fill a field shared with active scanning and
initiating. A passive scanner never transmits its own address, and the open
standalone profile starts with an empty resolving list and keeps address
resolution disabled. Its semantic descriptor therefore carries the same zero
entry link without executing a lookup whose result cannot be consumed. The
accelerator transaction remains required later for privacy-capable active
scanning and initiating, where the HAL writes it over the generated field
accessors rather than recreating it above the HAL.

Starting from the open driver's zero-based private allocation, its restricted
image has the following link-state words. This is the open SRAM-codec input.
Comparison with the current vendor standalone profile remains unqualified,
including the changed private `+0x18` and `+0x48` transforms:

| Link-state word | Open passive-1M image |
| ---: | ---: |
| `+0x00` | `0x1ff00000` |
| `+0x08` | `0x4ff00000` plus the bound RX-head low-20-bit link |
| `+0x0c` | `0xa0100000` |
| `+0x14` | `0x04000000` |
| `+0x18` | `0x40000000` |
| `+0x24` | `0x01100000`; the low-20-bit resolving entry remains zero |
| `+0x2c` | `0x00555555` |
| `+0x30` | `0x00001e00` |
| `+0x34` | zero |
| `+0x38` | `0x8e89bed6` |
| `+0x48` | `0x00000200` for the reviewed standalone option profile |
| `+0x50` | `0x03000000` for the reviewed standalone option profile |
| `+0x60` | provider-table index of the default power in byte `+0x61` |

All omitted words remain zero. The open memory layer publishes none of these
integers: callers provide only a bound memory graph, signed default power and a
typed controller-time observation. The private codec owns every shift, mask
and positional constant.

The first scheduler item is bounded as well. Complete
`r_ble_lll_scan_restart` selects primary channel 37, and the already reviewed
`r_ble_phy_chan_to_freq` mapping lowers it to frequency image zero. The
selected LE 1M mode passes through `r_ble_phy_rate_to_phy` and
`r_ble_phy_mode_to_rate` to rate image zero in both replicated scheduler
lanes. The item carries scanner kind two, a bounded start/end window, the
rounded-power projection copied from the link state and the existing common
scheduler insertion links. Channels 38 and 39 are subsequent events using the
same typed transform with the already reviewed primary-channel frequency
images 24 and 78; they do not require new MMIO operations.

For the restricted passive path, the complete named body selects the RX event
branch, clears both replicated rate lanes for LE 1M, binds the selected
frequency, copies the link-state rounded-power projection, stores the bounded
start/end ticks and records the window in the link state. The pinned
`r_ble_lll_scan_restart` (`r_sym_ble_M0sTWGzdUqAUyXoK849F`) stores
`r_sched_timer_convertDiffToTicks(window end - start)` at `+0x34`, clears
bit 20 of `+0x18` and ends the item at most 32768 microseconds after the
start; for a continuous scan it stores the difference `0x3fffffff` instead.
The open scanner always schedules finite windows and follows the first
branch. Like every other role's item, the scanner item carries the common
`r_btdm_sched_calc_seq_time` projection: its sequence starts one sequence lead
after the item start and lasts the item window. Hardware ends an item without
a sequence at once with no receive, which on the stand showed as scanner items
completing within 200 microseconds of insertion. The same body sets bit 23 of item `+0x00` and writes the default
arbitration priority in `+0x18` bits 3:0: 4 when bit 16 of the window's start
time is set, 1 otherwise, clearing bits 7:4. The open codec always writes 1.
That nibble is not consulted on any reachable path of the pinned scheduler (see
[the scheduler lists](bluetooth-scheduler-lists.md#overlap-check)), so this
alternation has no effect there; whether hardware reads the nibble is not
established. Its
named `r_ble_lll_scan_get_earliest_start_time` result selects the sole
adjusted-start item flag. The open codec accepts that semantic result rather
than exposing the positional flag or reproducing the vendor timing policy.

The allocation-time item prefix is mandatory before that event transform.
The complete common scheduler allocator supplies `+0x1c` with its bit-21
default cleared and a positional `+0x24` lane image that the coexistence
lanes below replace. The complete scanner
allocator then clears the high nibble of `+0x1c`, derives each item's low
twelve bits at `+0x20` from the public extended-advertising-instance and
connection limits plus one and the zero-based item index, and writes one at
`+0x2c`. The restricted legacy path has its extended-scan selector clear, so
the additional extended-scan-only `+0x1c` transform is absent. These fields
are installed by the private graph codec from a checked semantic allocation
configuration; callers cannot provide any positional word.

Item `+0x24` holds four five-bit coexistence priority lanes in bits 19:0. The
open graph writes them for every window instead of keeping the allocator's
image. With another radio on the antenna the vendor applies dynamic priority
control. `coexScan.c.o_1.o` `r_sym_coexScan_FGUQNnreeyQiPkw2qTYu` installs a
dynamic lane table, with columns selected by the scanner's time-based state,
and a short default lane table. The scan PTI initializer
`r_sym_coexScan_s7w1EV32meG8f6y0sPBq` then gives a passive scanner (scan type
zero at the selected PHY's `+0x04`) its default lanes; only lanes zero and one
are non-zero. For each window, `r_ble_lll_scan_restart` calls
`r_sym_coexScan_wNFqQvVjWMhmRY8ZGk4o`, which on a primary channel replaces
lanes zero and one from the dynamic table. Its time-based state only selects
between equal table columns and so does not change the lanes. The concrete
lane values are to be measured on the stand: dump item `+0x24` from a running
vendor image under shared-radio load. Alone on the antenna,
every lane requests 15; this equal standalone policy is a product choice.

The selected-item ownership edge is also explicit. `r_ble_lll_scan_restart`
loads the full scheduler free head from link state `+0x64`, derives its
compressed predecessor from item `+0x00`, and leaves both links unchanged when
common insertion reports the retryable collision result `-2`. Only a successful
insertion advances link state `+0x64` to that predecessor and retains the
selected item as active. The open graph performs the equivalent detach before
MMIO as a cancellable CPU transition: cancellation restores both links, while
RX-list and scan-command publication make the detached state irreversible.
This avoids exposing a vendor intrusive-list ABI and guarantees that RUN never
sees the active item still reachable through the private free chain.

## Open architecture

The first implementation should contain these owners, in this order:

1. a private, pinned scanner graph with affine CPU-owned, published, running,
   completed and reclaimed states;
2. a HAL scan-start transaction and stable read accessor for the
   existing scan hardware snapshot;
3. a controller role that schedules only passive LE 1M windows on channels
   37, 38 and 39 and uses the existing list/interrupt runtime;
4. a portable LL parser that accepts bounded legacy advertising PDUs and owns
   duplicate-filter policy independently of the hardware codec;
5. the existing `bt-hci` command/event types for LE Set Scan Parameters, LE Set
   Scan Enable and LE Advertising Report.

No upper layer may construct register images.  No public LL type may contain
the vendor link-state words.  SRAM masks remain private implementation details
of typed memory accessors, just as for the DTM descriptors.

The radio role admitted each scan window against one fresh Controller-time
sample; the core placed it apart from every other window with the
reservation function the radio shares, and the radio never moves it; only the
admitted interval is encoded into SRAM. The scanner item then stays with the
executor from list-zero insertion through the selector-one receive chain,
`RUN`, fenced finished-list capture and the completion walk, which returns the
item and let the role copy the PDU/RSSI results out of the scanning chain. A
cancelled window leaves through the executor's cancellation. The Link Layer
core that owns scanning policy and HCI routing does not exist yet; this lower
contract does not establish its readiness.

## Filter accept list

The filter accept list and the resolving list share one device table in
Controller SRAM that hardware walks while it filters. Each 8-byte entry holds
list-membership flags in byte 0 (bit 1 filter accept list, bit 0 resolving
list), a valid flag (bit 7) and a random-address flag (bit 6) in byte 1, and
the address least significant octet first in bytes 2 to 7. The Controller
publishes the table's compressed first-entry address in
`BTMAC_BLE_PHY_INIT.DEVICE_TABLE_FIRST_ENTRY` and its entry count in the low
octet of `BTMAC_BLE_PHY_INIT.DEVICE_TABLE_CONTROL`, preserving the rest of
that register; the vendor writes the first-entry address even for an empty
list. The S31 build keeps 12 entries. The device table owner of an
implementation outside this repository keeps members packed from the first entry, so the count is the
number of listed devices; it holds no resolving-list entries.

## Scope boundary

The restricted profile uses LE 1M, a public address and filter policy 0 or 1. Its contract covers the link-state/scheduler projection, selector-one
RX routing, completion gates and packet length/address/RSSI locations. Active
scan request/response, extended PHYs, duplicate caching and vendor timer/callout
policy are not implied by this profile. The production controller owns role
composition and HCI delivery.
