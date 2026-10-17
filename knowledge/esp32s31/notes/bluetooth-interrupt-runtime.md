# ESP32-S31 Bluetooth interrupt contracts

This reference describes the pinned primary and NRT interrupt paths, their
MMIO ordering and deferred-work semantics. Vendor callback/RTOS topology is
comparison evidence; the Rust runtime owns its typed dispatch and lifecycle.
Source-124 command-ready causality is not established by callback topology.
The predecessor repository's radio runtime rechecked pending scheduler waits
after a bounded delay.

## Pinned inputs

- public Controller archive
  [`espressif/esp32s31-bt-lib@10c507788e9da0993709cf82e405c896561172d8`](https://github.com/espressif/esp32s31-bt-lib/tree/10c507788e9da0993709cf82e405c896561172d8),
  `libbtdm_common.a` SHA-256
  `389561cead8a68444b46118fb606d5742813c88f8a37c43d22be271aaa4bdb41` and
  `libble_app.a` SHA-256
  `e61c5f8b0e558df8c520bcedd78dd8b930c2b61350b08c03430b774275385723`, the
  single pin of `knowledge/esp32s31/vendor/artifacts.toml`, for the reference
  gate, the linear broker and the finished-list path;
- earlier public Controller archive, for the remaining rows:
  [`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31),
  `libbtdm_common.a` SHA-256
  `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099`;
- public BLE archive at the same revision, `libble_app.a` SHA-256
  `62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  initial `libble_app.a` SHA-256
  `ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`
  and initial `libbtdm_common.a` SHA-256
  `bd9007072c6ab94df5f29d8b96dc65a69cb4406568c75a64022c8121e242b96c`;
- public OSAL source:
  [`btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L139-L211),
  SHA-256
  `436d009f67fbe7c83fa367a6b80dbf87316727605e751febc03c5118a669b5c8`.

Complete function bodies were reviewed from temporary, hash-verified public
artifacts. Neither object files nor disassembly products are repository
inputs. The table below records only the distilled control and MMIO facts.

| Body | Size | Recovered contract |
| --- | ---: | --- |
| `r_sym_bt_R9GZfnUbtn7k6mHtoZbv` | 186 bytes (10c5077) | Primary ISR: masked sample, shared W1C acknowledgement, HAL broker entry 0, dynamic scheduler suffix, HAL broker entry 1. |
| `r_sym_bt_hkluARKZBNTcJqMeSNys` | 190 bytes | Primary fault prefix: tests exactly bank-zero source 15 and bank-one sources 9, 8 and 12 in that order; source 9 reads `0x2010_11c8/11cc` and source 12 reads `0x2010_1070` before asserting. |
| `r_sym_bt_OBoFn3bgoHEVQdNFtCsb` | 100 bytes (10c5077) | Linear broker dispatch: a table holds a halfword entry count and one pointer per entry to a halfword-counted callback list; it calls that entry's callbacks in order and stops when one returns nonzero. The tables are assembled at link time from `.subinfo` descriptors whose first byte is the entry index, replacing the run-time registration walk of 7f20740. |
| `r_sym_bt_MDZbajeLxsB0cvuBOREd` / `r_sym_bt_2VbAPsx8lRNZdbVZTEOe` | complete leaves | Read `0x2010_105c/1068`, then copy both images to `0x2010_1058/1064`. |
| `r_sym_bt_6lAYUFKOuBLyOZ6Kvsv5` / `r_sym_bt_Ak3CRkSbyZRhUlneqclG` / `r_sym_bt_DOVkQWJHjeuid8jcS9Bq` | 24 / 24 / 16 bytes | Enable, disable and W1C the exact dynamic images `0x1820_0000` and `0x0000_0008`. |
| `r_sym_bt_37HcX0qW6j1XVtKakUIG` / `r_sym_bt_zczKhmPr5kLPCXpBc7GE` | 80 / 92 bytes | Decode `SCHEDULER_STATE`; the boolean consumed by deferred-work construction is exactly bit 31 AND bit 29. |
| `r_sym_bt_iFvwGI2tL5M1WM3fIkHq` | 12 bytes | Instruction-identical to same-chip named `r_btdm_sched_get_current_link_index`: one `SCHEDULER_STATE` read returns exactly the zero-based hardware-list index in bits 23:20. |
| `r_sym_bt_iEsFo1nbR5S71P2lMKhY` / `r_sym_bt_gs5GeSH15pdzrMDbb7oK` | 90 / 78 bytes | Build one static deferred event, optionally set its one-bit argument, and optionally publish the decoded state through entry three of `_btdm_sched_linear_broker_ram`. |
| `r_sym_bt_uNi9OHmE7XdXfGqTelU5` | 106 bytes | Same-chip named `r_btdm_recycle_in_task`: consume and clear the marker, drain scheduler work, then dispatch entry two of the linker-assembled `_btdm_sched_linear_broker_flash` table with argument zero; this is a drain event, not one callback per hardware edge. |
| `r_sym_bt_E8c5Eimm0z6kYe9v4wHr` / `r_sym_bt_YRnBzKlWCjsIbotqvNyS` | 334 / 562 bytes (10c5077) | Insert a scheduler item through its `+0x54` intrusive link into the manager-root completion queue; task-side removal/reordering calls the insertion path. This proves a software producer, not a hardware completion FIFO. |
| `r_sym_bt_WHYoiw8ufY0AEM2KSRK1` | 122 bytes (10c5077) | Same-chip named `r_btdm_sched_pop_executed_sch`: on every worker pop attempt, copy the low halfword from `0x2010_125c` to a zero-high image at `0x2010_1260`, merge the pending finished-list mask, synchronously dispatch it through entry five of the scheduler flash broker table when nonzero and then attempt a software completed-list pop. |
| `brk_sym_sched_MzeSZzbQ4ZKhWW6Wu5jV` / `r_sym_bt_M9nG353V0svWrv1l1zGw` | 458 / 324 bytes (10c5077) | Same-chip named `r_sched_txn_onSchedHwListDone` and `r_btdm_sched_pick_finished_items`: walk set finished-list bits, unlink matching scheduler items from the hardware-linked list and append them to the software completed queue through `item+0x54`. The first is the BLE subscriber of scheduler flash broker entry five; the BR/EDR subscriber of the same entry is the unobfuscated `btdm_common_sched_bredr_on_sched_hw_list_done`. |
| `r_sym_bt_QsLKLOCC2pct4rL8uFBN` | 130 bytes | Same-chip named `r_btdm_recycle_process_dequeued_sch`: clear the dequeued link image, recover its link-state pointer and invoke the item-specific recycle callback stored at `item+0x58`. |
| `brk_sym_sched_hOsJ4rGq1XjiY7GjHycG` | 176 bytes (10c5077) | Subscriber of entry five of the linker-assembled `_btdm_sched_linear_broker_ram` table, which `r_btdm_sched_check_remaining_entries` (`r_sym_bt_JlzTzVlQIZe4wkwwENZs`) dispatches; it checks the active BLE scheduler entries and asserts on an inconsistent one. |
| `brk_sym_scan_tdX6OLWyRKYxelkB8vZ1` | 10c5077 | Subscriber of entry three of `_btdm_sched_linear_broker_ram`, which `r_btdm_recycle_in_isr` (`r_sym_bt_iEsFo1nbR5S71P2lMKhY`) dispatches. It calls `r_sym_ble_Cnmfhv0PX8NAIxKVSVUs` and `r_ble_lll_scan_chk_resume` (`r_sym_ble_jQKuzvSTa70k58IPazik`), which retries the scanner restart while it returns `-2`, increasing the delay in steps of 100. |
| `r_sym_nrtIsr_kgxM2CJfVperrlVjmB4B` | 70 bytes (10c5077) | NRT ISR: raw sample at `0x2010_1340/1348`, shared W1C acknowledgement, then entry zero of the linker-assembled `_nrtIsr_linear_broker_flash` table with the two-word snapshot. No pinned archive member subscribes to that table, so the dispatch has no callback. |

## Exact primary suffix

The complete primary handler first preserves the implemented
sample/sample/ack/ack order. Its next complete function treats precisely the
baseline enable groups as fault/assert paths:

| Pending source | Diagnostic capture before assertion |
| --- | --- |
| bank 0 source 15 | none |
| bank 1 source 9 | complete words `IRQ_DIAGNOSTIC_DETAIL_0/1` at `0x2010_11c8/11cc` |
| bank 1 source 8 | none |
| bank 1 source 12 | complete `IRQ_DIAGNOSTIC_STATE` at `0x2010_1070` |

This is not a Link-Layer completion group. The restricted PAC therefore
projects the acknowledged status into one semantic
`BluetoothPrimaryInterruptEpoch` and never exports the raw bank or diagnostic
images. `BluetoothPrimaryFaultSources` also marks any pending status outside
the reviewed dynamic and fault source groups as unclassified. The Bluetooth
classifier gives known fault lanes and unclassified status precedence over
simultaneous dynamic bits and returns `PrimaryControllerFault`; the
live owner must retain it, skip ordinary LL work and enter
fail-stop/quiesce. Rust does not reproduce the vendor assert routine in
hard-interrupt context.

Only after that prefix does the reference handler dispatch entry zero of
`_btdm_hal_linear_broker_ram` with the two-word snapshot, execute the following
dynamic branch, then dispatch entry one with the same snapshot.

Dynamic classification is positional because Link-Layer names are not yet
proven:

| Pending image | First work input | State publication requested | Extra reference gate |
| --- | ---: | ---: | --- |
| bank 1 source 3 | bank 0 source 27 OR 28 | yes | yes |
| no bank 1 source 3; bank 0 source 27 OR 28 | yes | bank 0 source 21 | no |
| only bank 0 source 21 | no | yes | no |
| none of these sources | no work | no | no |

All three dynamic classes ultimately enqueue the same static recycle event.
The task callback always drains scheduler work for both ordinary and marked
batches; the marker controls only an additional buffer-recycle suffix. Source
27 and source 28 are collapsed into one boolean before this boundary and
cannot be named individually from the reviewed software. The raw ISR never
reads `SCHEDULER_FINISHED_LIST_STATUS`; that task-side read belongs exclusively
to the later worker. Consequently a source-124 batch is an affine permission
to make one fresh scheduler observation, not proof that any particular raw
source completed a hardware list.

Bank 1 source 3 has precedence over the bank-zero branch. Its reference gate
takes a diagnostic BUSY sample: it writes the complete selector image `0x38` to
`DIAGNOSTIC_SELECT` at `0x2010_11e8`, reads `DIAGNOSTIC_VALUE` at
`0x2010_11ec` until two consecutive complete reads are equal and tests bit 7.
When that bit is clear it writes zero to `SCHEDULER_REFERENCE` at
`0x2010_1078` and calls `r_btdm_sched_check_remaining_entries`, which
dispatches entry five of `_btdm_sched_linear_broker_ram`. Its BLE subscriber
`brk_sym_sched_hOsJ4rGq1XjiY7GjHycG` immediately performs a scheduler
transaction/list consistency action. The typed classifier therefore returns
`ClearReferenceAndContinue`. The callback has no MMIO or hardware mutation: it
walks and asserts over the vendor's private intrusive transaction nodes. The
open DTM scheduler does not create that container; its sole-item lifecycle is
affine, so reproducing that vendor callback would
add an artificial runtime ABI rather than a hardware invariant. Any
dynamic branch then makes a later, independent `SCHEDULER_STATE` read. The
derived reference state is `bit31 && bit29`. Deferred work is marked only when
the first work input and that derived state are both true. Entry three of
`_btdm_sched_linear_broker_ram`, dispatched by `r_btdm_recycle_in_isr`,
receives the same derived state when the table requests state publication; its
BLE subscriber uses both its presence and boolean payload to decide whether to
retry scheduler work.

The reviewed current-link leaf proves that bits 23:20 of this same register
are the zero-based current hardware-list index. The restricted PAC projects
that field from the later sample and the durable primary scheduler event
retains it beside the busy/reference observation. This identifies the active
list at that temporal point; it does not identify an affine scheduler item,
imply that the list is finished or authorize descriptor reclamation.

The two reads cannot be folded into one observation: the reference clear and
entry-five software path occur between their positions. The Rust classifier
therefore uses distinct reference-gate and work-observation types.

Each primary service publishes one classified scheduler wake into a single
coalescing cell. The wake carries no observed value: every executor wait takes
its own fresh joint observation, so a stored BUSY sample from an earlier
interrupt never decides a later transaction step. The runtime rechecks a
pending wait after a bounded delay even without a wake, because the vendor
bodies directly re-read BUSY and the command fields and prove neither
command-ready-to-source-124 causality nor a hardware retry wake.

## Event multiplicity and RTOS-free replacement

The exact public OSAL refuses a second insertion while the same static event
is queued. The marker assignment occurs before insertion, so a later marked
edge upgrades an already queued ordinary event, and a later ordinary edge
cannot clear a marker. Dequeue clears the queued flag before the event handler
runs. Consequently the required deferred contract is:

1. the first publication opens a pending epoch and wakes the sole worker;
2. repeated publications coalesce into that epoch;
3. `marked` is sticky across every coalesced publication;
4. dequeue atomically consumes pending plus marker;
5. a racing publication after dequeue opens a new epoch and emits a new wake.

`SchedulerWakeCell` implements this contract with one `AtomicU8`.
It is not an async primitive by itself: the platform forwarded each
`WakeWorker` publication to the predecessor repository's radio runtime signal,
and that runtime drained the real scheduler list before considering the epoch
complete; a new driver keeps that order.

The drained list is not a hidden hardware FIFO. Complete producer review shows
that task-side scheduler item removal/reordering inserts items into a
manager-root intrusive completion list through the link at `item+0x54`. On
every worker pop attempt, `r_sym_bt_WHY...` performs the low-halfword transfer
from `0x2010_125c` to `0x2010_1260`, merges the pending finished-list mask and
synchronously dispatches it through entry five of the scheduler flash broker
table. That entry's `.subinfo` descriptors bind it to `brk_sym_sched_MzeS...`,
which walks set list bits and calls
`r_sym_bt_M9n...` to move finished hardware-linked items onto that same
software queue. The worker then pops an item, and `r_sym_bt_QsLK...` invokes
its role-specific callback at `item+0x58`. For DTM this is the mapped
`r_ble_lll_dtm_recycle_sch_item` body. Thus the lock/modify request result is
not a completion signal; finished-mask selection plus item recycle is the
reference software ownership-return path. The binary analysis does not label
the broker delivery complete until subscriber lifetime is joined to the
publisher epoch and every preceding listener is proven to continue for this selector, or the
exact broker epoch is replayed.

The open scheduler therefore defines its own typed item lifecycle, with an
explicit hardware-finished to CPU-readable fence. Reproducing the vendor list
nodes or OSAL event object is neither required nor desirable. The restricted
PAC names and transfers the exact 16-bit `SCHEDULER_FINISHED_LIST_STATUS` mask
while keeping the destination word positional as
`SCHEDULER_FINISHED_LIST_REPORT`. The Bluetooth core drains one list bit per
bounded step; a list-zero bit runs the executor's completion walk, which
returns the executed items at the head of the list mirror to their role. The separately retained current hardware-list
index and every finished-list bit use the same `0..16` hardware-list domain.
A finished bit identifies a list, while the current index identifies the
active list at a distinct temporal point; neither selects an affine item.

The pinned archive has no run-time registration: every consumer subscribes to
an entry of `_btdm_sched_linear_broker_ram` through its `.subinfo`
descriptor. Entry three, dispatched from `r_btdm_recycle_in_isr`, reaches
the scanner resume `brk_sym_scan_tdX6OLWyRKYxelkB8vZ1`; entry five, dispatched
from `r_btdm_sched_check_remaining_entries`, reaches
`brk_sym_sched_hOsJ4rGq1XjiY7GjHycG`. Entries one and two, dispatched from the
scheduler run body and `r_btdm_sched_stop`, reach the scanner subscribers
`brk_sym_scan_b8ogEC3hTjGmqL5oRuyw` and `brk_sym_scan_fd4yzdFac8vdFTlQ4BPA`;
entry zero, dispatched from `r_btdm_sched_actual_start_time_get`, reaches
`brk_sym_sched_PNMeumLUZPcz8o4UrTLi` and an ISO subscriber. These entries are
replaceable software ABI, but their applicable effects are not optional. The
C61-named `r_ble_lll_scan_chk_resume` classifies the 102-byte pinned
`r_sym_ble_jQKuzvSTa70k58IPazik` as scanner-role resume. An open DTM-only
scheduler therefore needs no scanner-resume subscriber; a later scan ULL owner
must provide the typed collision/resume policy. The entry-five check remains
evidence about the vendor's active-transaction container, but has no runtime
successor in the open scheduler: the corresponding list consistency is
structural in its affine owners. The current wake cell retains
only the proven pending/marked coalescing contract; it intentionally does not
pretend to implement either higher-level action. The DTM-specific consequence
and evidence chain are recorded in
[`bluetooth-direct-test-mode.md`](bluetooth-direct-test-mode.md).

The linked callback selectors are boundaries between closed vendor software
components, not ESP32-S31 hardware ABI. An open Link Layer may use its own
typed events. It must nevertheless preserve every proven MMIO ordering edge,
the sticky marked-work distinction, and any behavior later shown to be
required by the scheduler-list consumer.

## NRT default-lifecycle contract

The NRT ISR dispatches entry zero of the `_nrtIsr_linear_broker_flash` table
synchronously after raw sample/sample/ack/ack. The linker assembles that table
from `__sublinear__F_nrtIsr_linear_broker_flash` descriptors. No member of the
pinned BLE, common or classic Controller archives defines one, while the other
broker tables do have subscribers, so the absence is not inferred merely from
stripped names. The dispatch therefore reaches no callback in the pinned
controller.

This establishes an acknowledge-only NRT path for the pinned default software
lifecycle, not a silicon guarantee that source 133 can be removed or that its
snapshot fields have no meaning under future features. The open ISR must retain
the affine acknowledgement token and shared-clear ordering. It must not invent
LL wakes until a feature-specific producer/consumer path or a hardware test
establishes them.

## Claim boundaries

The pinned default NRT callback graph establishes acknowledge-only behavior
for that composition. Feature-specific status meanings and mask/re-arm policy
require their own evidence. Primary publication, scheduler memory ownership,
executor notification and quiescent route teardown are separate owners; a
callback selector is not a silicon interrupt ABI. Runtime support and
hardware readiness are described by their respective driver and test owners.
