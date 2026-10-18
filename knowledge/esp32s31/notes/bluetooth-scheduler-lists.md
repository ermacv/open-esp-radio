# ESP32-S31 Bluetooth scheduler list contracts

This reference describes how the vendor Controller keeps several scheduler
items and several hardware lists live at the same time: list ownership,
time-ordered insertion, conflict resolution, insertion into a running list and
completion. DTM descriptor lowering is described in
[Direct Test Mode](bluetooth-direct-test-mode.md); the interrupt and deferred
work path is described in [Bluetooth interrupts](bluetooth-interrupt-runtime.md).

The open scheduler executor implements list zero from these bodies: an ordered
mirror with strict wrapping overlap rejection, idle insertion, the live
insertion bracket, two-phase cancellation with the skip hold, list deletion,
the completion walk and the stopped state. Its hardware execution retains each
command publication until the action that ends it and takes one fresh
observation per wait. Conflicts are rejected before submission rather than
moved. The open contracts at the end of this page remain hardware questions.

## Pinned public inputs

- ESP32-S31 Controller archive
  [`espressif/esp32s31-bt-lib@10c507788e9da0993709cf82e405c896561172d8`](https://github.com/espressif/esp32s31-bt-lib/tree/10c507788e9da0993709cf82e405c896561172d8),
  the single pin of `knowledge/esp32s31/vendor/artifacts.toml`:
  `libbtdm_common.a` SHA-256
  `389561cead8a68444b46118fb606d5742813c88f8a37c43d22be271aaa4bdb41`
  (scheduler member `18.o`) and `libble_app.a` SHA-256
  `e61c5f8b0e558df8c520bcedd78dd8b930c2b61350b08c03430b774275385723`
  (member `sched_3.o`);
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  `libbtdm_common.a` SHA-256
  `bd9007072c6ab94df5f29d8b96dc65a69cb4406568c75a64022c8121e242b96c`
  (member `btdm_sched.c.o`) and `libble_app.a` SHA-256
  `ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`
  (members `sched_txn.c.o`, `sched_stack.c.o`).

Every behavioral statement below comes from the current-revision instruction
body. Names come from the initial archive and are carried through every
intermediate archive revision by the predecessor repository's
`oer-tool-service-symbol-lineage` tool: a name
crosses revision `5e37d4d`, which introduced generated names, on its unchanged
source name or on an identical relocation-normalized body, and later revisions
keep generated names stable; the pinned `libble_app.a` renamed the scheduler
transaction functions again, and the tool pairs them by body and call graph.
The last column gives the weakest evidence step of each name's chain. Every
body was read in its pinned form.

`r_sym_bt_DPWY0umixzmXEaFuUyCI` is `r_btdm_sched_run`: its revision-`5e37d4d`
body is identical to the initial one. Revision `7729629` moved that body into
the new function `r_sym_bt_PVKilXLQPu1BjRkm4C6O`, which has no recovered name,
and made `r_btdm_sched_run` its caller.

| Initial name | Current symbol | Name evidence |
| --- | --- | --- |
| `r_btdm_sched_insert_with_lock_modify` | `r_sym_bt_VrTmsQfPlkmys4UL0NZp` | exact body |
| `r_btdm_sched_insertion_begin` | `r_sym_bt_EabYtUaAIR05LXw3qZSA` | exact body |
| `r_btdm_sched_insertion_end` | `r_sym_bt_4KfpZh0Hu5NprlqcNu0D` | exact body |
| `r_btdm_sched_execution_lock` | `r_sym_bt_9H3AnHbaHJ3auzSvPDme` | exact body |
| `r_btdm_sched_execution_modify` | `r_sym_bt_rPoPGH6BBYjZaunDU5FV` | exact body |
| `r_btdm_sched_wait_lock_modify_idle` | `r_sym_bt_B8fDByebTHuwRe8FZuRt` | exact body |
| `r_btdm_sched_merge_list_remove_overlap` | `r_sym_bt_YRnBzKlWCjsIbotqvNyS` | call graph |
| `r_btdm_sched_check_overlap_in_list` | `r_sym_bt_FovcDCPDYkKMaCv7y4Wb` | exact body |
| `r_btdm_sched_remove_unshareable_entries` | `r_sym_bt_E8c5Eimm0z6kYe9v4wHr` | exact body |
| `r_btdm_sched_rm_item_directly` | `r_sym_bt_hddDtuOCoB0U4KYRErRq` | exact body |
| `r_btdm_sched_delete_from_list` | `r_sym_bt_KmzLfKJ5UUi7zkqz1Nwn` | exact body |
| `r_btdm_sched_delete_specified_items` | `r_sym_bt_qkNMymdnaJnYUfzpKgEp` | exact body |
| `r_btdm_sched_search_deleted_items` | `r_sym_bt_1Q7VVJH4siSgF2fbAf3f` | exact body |
| `r_btdm_sched_skip_specified_sch` | `r_sym_bt_DkrQYQcoyzIHdYi0CzWE` | similar body |
| `r_btdm_sched_stop` | `r_sym_bt_74l62ZLsZuXg67pPHSd7` | exact body |
| `r_btdm_hal_link_skip_specified_tl` | `r_sym_bt_t4aeyhcVrKTNMSlq45XR` | exact body |
| `r_btdm_sched_pick_finished_items` | `r_sym_bt_M9nG353V0svWrv1l1zGw` | exact body |
| `r_btdm_sched_actual_start_time_get` | `r_sym_bt_O5lSxcYW6bGB7SPJpXgX` | lineage |
| `r_btdm_sched_arbitrate_by_default_policy` | `r_sym_bt_jzw6WTDKUlY3F90cFtr2` | lineage |
| `r_sched_txn_delayAccordingToPriorityCb` | `r_sym_sched_IQtKTdW2DgEK4Oo4n10P` | lineage |
| `r_sched_txn_onSchedGetEarliestTicks` | `brk_sym_sched_wfZseauvjWnjfiu10yWN` | lineage |
| `r_btdm_sched_run` | `r_sym_bt_DPWY0umixzmXEaFuUyCI` | exact body |
| `r_btdm_sched_get_hw_list_header` / `set_hw_list_header` | `r_sym_bt_6wSHUtNRioHeB7CKjVJA` / `r_sym_bt_8m3cRMNRZNfaJ7qVvayk` | exact body |
| `r_btdm_sched_mem_get_hw_start_time` | `r_sym_bt_sdf6bUMpe1CnARWl962a` | exact body |
| `r_btdm_sched_reset_new_item` | `r_sym_bt_RnJIqDW4oA0usCDLVZGy` | exact body |
| `r_btdm_sched_replace_hw_entry_with_index` | `r_sym_bt_vibwBXogwGafrHSYUbon` | exact body |
| `r_sched_txn_insertIntoList` | `r_sym_sched_OAGUngJKx5kn6bwQXdLx` | similar body |
| `r_sched_txn_insertOne` | `r_sym_sched_mYA9XxLdNJrKN52WzuzL` | similar body |
| `r_sched_txn_getListFromSch` / `getListFromDevice` | `r_sym_sched_M7rlJqpeQjP02yIEpjWX` / `r_sym_sched_RIkbUZx5qxZKaT5j7dWZ` | similar body |
| `r_sched_txn_resortSwList` | `r_sym_sched_IuRfwmqxkZQzOfkj2sZX` | call graph |
| `r_sched_txn_removeSwList` | `r_sym_sched_4yOHdQa50AXnZAom3x3f` | call graph |
| `r_sched_txn_onSchedHwListDone` | `brk_sym_sched_MzeSZzbQ4ZKhWW6Wu5jV` | dominant candidate |
| `r_sched_txn_init` | `r_sym_sched_TtqwTBf6GWrS99gJ5I6I` | call graph |

## Hardware list model

The scheduler exposes sixteen hardware lists. List `i` occupies the two words
at `0x2010_b000 + 0x10 * i`:

- word 0 bits 19:0 hold the compressed head item pointer. Its address is
  `0x2f00_0000 | (field << 2)`; a zero field is an empty list. Writers preserve
  bits 31:20;
- word 1 is the hardware start time of that list.

A list is a singly linked chain through item word `+0x00` bits 19:0, using the
same compression. Walkers OR the shifted field with the constant `0x2f00_0000`
to recover a full address. `SCHEDULER_STATE` at `0x2010_107c` reports the list
being executed in bits 23:20. Every scheduler function takes BUSY from the
MAC diagnostic pair instead of that register: it writes the complete selector
image `0x38` to `0x2010_11e8`, reads `0x2010_11ec` until two consecutive
complete reads are equal and tests bit 7. BUSY below always means this
sample. The vendor loop is unbounded. Here one PAC call is one selection and
two reads, and the HAL repeats it at most `DIAGNOSTIC_READ_BUDGET` times (the
driver's attempt budget); the Controller treats an exhausted budget as a
scheduler fault.

The BLE stack reserves thirteen lists: `r_sched_txn_init` stores 13 as the
list count and `0x1fff` as the free-index bitmap. Hardware indexes are a
limited resource that the software lends to lists, not a fixed per-role
assignment.

## Scheduler item fields

These offsets are used by the list functions. They extend, and do not
replace, the role-specific item layouts in the role references.

| Offset | Meaning in list code |
| --- | --- |
| `+0x00` bits 19:0 | Hardware next link. `reset_new_item` clears it together with bit 25 |
| `+0x00` bit 25 | Set on every deleted, skipped item, and on a preempted item when lock-modify is enabled |
| `+0x00` bit 24 | Marks the item that the merge reports to its optional first-marked output |
| `+0x00` bit 22 | Flexible start: the item may start once its predecessor ends. When clear, a completed item must not start after the current list head |
| `+0x18` bits 3:0 | Default-arbitration priority (not consulted on any reachable conflict path) |
| `+0x30` bit 31 | Selects the device list; when clear, the overlap check applies the late-start check |
| `+0x38` | Execution status. `0xffff_ffff` means not executed; hardware replaces it; preemption writes zero |
| `+0x44` / `+0x48` | Start and end time; lists are ordered by start |
| `+0x4d` | Item kind; kind 2 is a background item handled by a separate path |
| `+0x4e` | Halfword flags. Merge clears the low byte; reset sets bit 0 and clears bits 10:8 |
| `+0x4f` bit 3 | Overlap already resolved; merge inserts by time without removing entries |
| `+0x4f` bits 2:1 | Set to `0b11` when the item is preempted; bit 1 alone marks a deleted item |
| `+0x50` | Previous item in its list, and the caller's insertion hint |
| `+0x54` | Software link for a chain of new items and for the completion queue |

## Software lists

A software list descriptor is `0x1c` bytes:

| Offset | Meaning |
| --- | --- |
| `+0x00` | Earliest item that must be considered current |
| `+0x04` / `+0x08` | Head and tail of the item chain |
| `+0x0c` / `+0x10` | Links in the start-ordered list of software lists |
| `+0x14` | Start time used to order software lists |
| `+0x18` | Signed hardware list index; negative means software-only |
| `+0x19` | List enabled |
| `+0x1a` | Start time comes from the sleep timer instead of the list word |

The stack owns a default list shared by all ordinary roles and a background
list. An item whose `+0x30` bit 31 is set belongs to its device's embedded
list instead (device `+0x2c`, selected by device `+0x04` bit 27). The Direct
Test Mode context embeds such a list with index 0.

`r_sched_txn_resortSwList` keeps software lists ordered by their start time and
lends the free hardware indexes to the earliest lists. When all indexes are
lent, the index of the latest list is moved to the earlier one through
`r_btdm_sched_replace_hw_entry_with_index`. Items of different roles therefore
share one hardware list, in start order, while their software list holds an
index. Arbitration between hardware lists that are live together is performed
by hardware; the criterion it applies is not established by these bodies.

## Insertion

`r_sched_txn_insertOne` first runs `r_btdm_sched_check_overlap_in_list`, then
`r_sched_txn_insertIntoList`. That calls `r_btdm_sched_calc_seq_time` and, for an
enabled list with a hardware index, `r_btdm_sched_insert_with_lock_modify`. A
software-only list uses the merge alone. When the merge makes the item the new
head, the list start time is refreshed and the software lists are resorted.

### Overlap check

`r_btdm_sched_check_overlap_in_list(list, item, hint, priority, argument)`
decides whether a new item may enter a list and after which entry.

- Admission: an item outside a device list (`+0x30` bit 31 clear) must start
  later than the current time plus the stack margin; otherwise the result is
  -2. An admitted item yields 0, 1 or 2.
- Marking: the new item leaves with `+0x4f` bit 3 set, the mark that the merge
  reads as "may share time with others".
- Predecessor: the search follows the hardware links from just after the
  latest predecessor of the hint whose `+0x4e` byte is zero, or from the
  list's current item. The predecessor is the last entry that ends before the
  item starts; an entry that starts after the item ends closes the search.
  Intervals are `[start, end)`.
- Overlaps: an optional `priority(item, entry, argument)` callback judges each
  overlapping entry. A negative verdict rejects the item, 2 tolerates the
  overlap and 1 tolerates it without making the entry a predecessor. With
  verdict 0 or without a callback the overlap is tolerated, because the item
  already carries bit 3.
- Output: item `+0x50` names the predecessor; the item's hardware link and
  flags and its `+0x54` are reset.

The priority nibble comparison after a missing callback is therefore
unreachable in this body.

The only other reader of the nibble is `r_btdm_sched_arbitrate_by_default_policy`
(`r_sym_bt_jzw6WTDKUlY3F90cFtr2`). It returns 2 when either item has `+0x4f`
bit 3 and otherwise ranks the higher nibble first. Its sole caller,
`r_sched_txn_delayAccordingToPriorityCb`, passes the new item first, which the
overlap check has already marked, so this comparison is unreachable as well.
The nibble is therefore a software priority of a dormant default policy;
whether hardware reads it is not established.

### Flexible start

Item `+0x00` bit 22 is written only by the advertising follower chain
(`r_ble_lll_adv_sched_remaining_pri_after`, pinned
`r_sym_ble_eNifqLwR78cnxeKb1y6t`) and by `r_ble_lll_scan_restart` when
`r_ble_lll_scan_get_earliest_start_time` reports an adjusted start. The first
advertising item clears it. `r_sched_txn_onSchedGetEarliestTicks` (pinned
`brk_sym_sched_wfZseauvjWnjfiu10yWN`) reads it on the head item of the first
software list. When it is set, the function reports that the head has no
fixed start. Otherwise it reports the item's actual start time from
`r_btdm_sched_actual_start_time_get`, which returns `+0x44` for an item with
`+0x30` bit 31 clear. The names and uses identify the bit as a flexible start
that follows the predecessor. The hardware effect is not established by these
bodies.

### Merge

`r_btdm_sched_merge_list_remove_overlap(chain, list, last, marked)` inserts a
chain of new items linked through `+0x54`. Each inserted item leaves with its
`+0x4e` byte clear and the unexecuted status `0xffff_ffff`. Where it lands
depends on its `+0x4f` bit 3:

- a shareable item (bit 3 set) follows the latest entry, at or after its
  `+0x50` hint, whose start is not later than its own;
- an unshareable item (bit 3 clear) first evicts the overlapping unshareable
  entries through `r_btdm_sched_remove_unshareable_entries` and follows the
  predecessor that call reports.

A placed item is linked in both directions (its own and its predecessor's
hardware links, the successor's `+0x50`), and the list's head, tail and current
item follow. A successor that has already executed is an assertion failure.
The result is 7 when an item became the list head and 0 otherwise; `last`
receives the final item of the chain.

An entry evicted by `r_btdm_sched_remove_unshareable_entries` is one that
overlaps the new item and lacks `+0x4f` bit 3. It is logged, unlinked, queued
for completion through `+0x54` and marked preempted: `+0x4f` bits 2:1 become
`0b11` and an unexecuted status becomes 0; when the environment enables
lock-modify it also gets `+0x00` bit 25. Preempted items therefore reach their
recycle callback through the ordinary completion path.

### Insertion into a live list

`r_btdm_sched_insert_with_lock_modify(list, item, index)` brackets the merge
with a hardware transaction:

| Begin outcome | Condition | Action |
| --- | --- | --- |
| 3 | sleep policy disabled | No list change |
| 3 | BUSY clear | Hint becomes the list tail; list current item is cleared |
| 4 | BUSY and the execution lock at the hint succeeds | Hardware is locked at the predecessor |
| 5 | BUSY otherwise | Execution modify on the list mask; hint becomes the predecessor of the hardware head; list current item becomes that head |

The sleep-policy predicate is the external function that the
[Direct Test Mode](bluetooth-direct-test-mode.md) reference identifies for
insertion end. The registers are:

- `0x2010_1254` execution lock: bit 31 START, bits 23:20 list index, bits
  19:0 compressed item. Bit 24 reports completion and bit 26 the lock engine
  idle. Bits 29:27 are the result: 0 locks, 2, 5, 6 and 7 fail, and 1, 3 and
  4 are treated as impossible;
- `0x2010_1258` execution modify: bit 31 START, bit 16 a mode flag, bits 15:0
  the list mask. Bit 17 reports completion and bit 18 the modify engine idle;
  bit 19 set is treated as impossible;
- `0x2010_1218` lock-modify request: bit 31 START and bits 19:0 compressed
  item, after the list index is written to the low nibble of `0x2010_136c`.
  Bits 30:27 return a result.

Both commands are issued only once the scheduler is idle or both engines
report idle. A lock is complete when the scheduler is idle or bit 24 is set.
A modify additionally watches two signals of the MAC diagnostic pair: a
progress signal (selector `0x0c01`), whose conflict states make the request
repeat, and a settle signal (selector 7), which must leave the value 9. A
repeated request is withdrawn by clearing START and issued again. The
observable protocol and the open questions are recorded in
[scheduler execution lock and modify](../../../knowledge/esp32s31/bluetooth/scheduler-execution-lock-modify.md).
Every vendor wait polls only while BUSY is set and asserts after 9999
iterations.

After the merge, outcome 4 also issues a lock-modify request for the last new
item when the environment enables it, and logs the result. Insertion end
closes the bracket: an outcome-4 lock and an outcome-5 modify are released by
clearing their START, and outcome 5 first makes the submitted item the
hardware list head. When BUSY is clear the hardware head becomes the software
list head with sleep policy disabled or, for an unexecuted submitted item,
that item, and `r_btdm_sched_run` starts the scheduler. That run acknowledges
and enables the dynamic interrupts and scheduler broker entry one (whose BLE
subscriber acknowledges and enables BTMAC source 14) through
`r_sym_bt_PVKilXLQPu1BjRkm4C6O`; when that returns zero, a 1 written to
`0x2010_1000` starts the scheduler.

## Completion

Hardware replaces an item's `0xffff_ffff` status. On a finished-list
interrupt, `r_sched_txn_onSchedHwListDone(mask)` visits every software list
whose hardware index is set in the mask:

1. it reads the list start time;
2. it calls `r_btdm_sched_pick_finished_items(list, index)`, which walks from
   the head, unlinks executed items and appends them to the completion queue.
   The walk skips at most one unexecuted item. A completed item without
   `+0x00` bit 22 must not start later than the current hardware head;
3. an emptied list must have an empty hardware head and is removed;
4. otherwise the list start time is refreshed and the software lists are
   resorted, provided that the hardware head is not empty, no out-of-order
   completion was found and `SCHEDULER_STATE` does not have bit 29 set while
   bits 23:20 name this list.

Queued items are then recycled through their `+0x58` callback, as described in
[Bluetooth interrupts](bluetooth-interrupt-runtime.md).

## Cancellation

Items to cancel are selected by `r_btdm_sched_search_deleted_items(start,
predicate, argument)`: a predicate over the hardware links from `start`
accepts an item into a chain linked through `+0x54`, passes over it or ends
the search. The `sched_txn` delete-by-type, by-state-machine and
caller-selection functions build such a chain for
`r_btdm_sched_delete_from_list`.

`r_btdm_sched_delete_from_list(list, index, chain)` begins with an external
sleep-path function. Without a chain it empties the whole list (head, tail and
current item). While the scheduler is idle the hardware head is simply
cleared; while BUSY the clear is bracketed by an execution modify on the list
mask with the mode flag set. With a chain it hands over to
`r_btdm_sched_delete_specified_items`.

`r_btdm_sched_delete_specified_items(list, index, chain)` cancels in two
phases and then notifies the recycle path:

- Software unlink. Each chained item is marked with `+0x00` bit 25 and
  `+0x4f` bit 1 and leaves the list's links, head, tail and current item. Its
  own hardware next link survives, so hardware that already holds it can
  still follow the chain.
- Hardware skip, for a list with a hardware index. While the scheduler is
  idle the hardware head is republished from the software head unless sleep
  policy is enabled. While BUSY, once the lock and modify engines are idle,
  the unexecuted chained items that do not start after the hardware head
  (read once, before the hold; every unexecuted item when it is empty) are
  skipped through `r_btdm_hal_link_skip_specified_tl`. Skip result 0 is
  treated as impossible, and results 2 and 4 end the skipping.
- Hold. When the environment enables lock-modify, the skips run inside a
  hold: the list index in `0x2010_136c` bits 7:4, the value `0x80` in
  `0x2010_1058` and `0x2010_1204` bit 0 set; the hold is established once
  `0x2010_1324` bit 0 is set and `0x2010_1208` bit 0 is clear, and is
  released by clearing `0x2010_1204` bit 0. It starts only after
  lock-modify START is clear.

`r_btdm_hal_link_skip_specified_tl(index, item)` is a single command in
`0x2010_10ec`: bit 31 START, the list index in bits 23:20 and the compressed
item in bits 19:0. The command is over when START or BUSY clears. Its result
is bits 29:28 of the register, or 4 when BUSY cleared first; the register is
then cleared.

`r_btdm_sched_skip_specified_sch(items, index, count)` applies the same skip,
with bit 25 on each item and the same stopping rule, to an array of items;
with lock-modify enabled the hold follows as a closing pulse.

`r_btdm_sched_stop` has nothing to do while the scheduler is idle. Otherwise
it disables the dynamic interrupts, dispatches entry two of the
linker-assembled scheduler broker table (the BLE subscriber clears BTMAC
source 14) and, once the lock and modify engines are idle, writes 1 to
`0x2010_1004`; BUSY still set after 65536 samples is an assertion failure.

## Receive routing

The named `ble_lll_mmgmt.c.o` and `ble_lll.c.o` of the role-name archive
route receptions per role:

- `r_ble_lll_mmgmt_update_global_rxlink(item)` enables the link state's
  receive object (link-state `+0x7c`), selects class one for scanner kind
  two and class two otherwise, writes the class to link-state `+0x20` bits
  30:28 and records item `+0x20` bits 11:0 in the object's `+0x18` halfword;
- the item numbers come from the role allocators: advertising instance `k`
  uses `k`, connection `c` uses the extended-advertising limit plus one plus
  `c`, the scanner's items follow the connections, and DTM follows five
  private and four further numbers plus the periodic-synchronization limit;
- `r_ble_lll_get_rxed_buffer(link_state)` walks the chain from the software
  head (link-state `+0x68`), stops at the first incomplete header, skips a
  completed packet whose `+0x18` halfword differs from the recorded number,
  moves a left-behind packetless header into the spare slot (`+0x78`) and
  marks the last completed header as the one hardware may still hold;
- `r_ble_lll_append_rx_buffer` returns a node after the tail (`+0x70`),
  copying a held header's packet into the spare header so that the held
  header stays in the chain without a packet;
- when the object is enabled for a global class, both functions first load
  and afterwards store the class's shared head, tail and spare, so every role
  of a class walks one chain;
- `r_ble_lll_conn_use_rxbuf_from_link_state` clears the class bits, keeps the
  object enabled with class zero and points the private consumer in
  link-state `+0x08` at the software head: a connection receives through its
  own chain.

## Open contracts

The bodies above do not establish:

- the hardware criterion between live hardware lists and whether hardware
  evaluates overlaps across lists;
- the effect of the execution lock, execution modify and lock-modify request
  on item execution, and the meaning of each result code;
- the modify mode flag, the lock-modify result and when the environment
  enables the lock-modify request;
- the hardware effect of item `+0x00` bit 22, the meaning of `SCHEDULER_STATE` bit 29 and of the
  sleep-timer start source;
- why insertion begin skips locking when sleep policy is disabled;
- the effect of the skip request, the hold at `0x2010_1204` and the meaning
  of the skip results 1 to 3;
- whether hardware reads item `+0x00` bit 25 as a skip marker;
- whether hardware records item `+0x20` bits 11:0 in received packet
  `+0x18`, and whether a class-zero link state receives only through its
  `+0x08` consumer;
- the sleep-path functions called by insertion and deletion, and the
  background list.
