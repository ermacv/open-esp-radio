# ESP32-S31 Bluetooth LE Direct Test Mode contracts

The open radio role, runtime and Controller core that this page compares
with the vendor are not in this repository; the passages that name them
describe an implementation outside this repository, which the memory
codecs, executor and portable pieces here follow.

This reference describes the DTM descriptor, scheduler, timing and ownership
contracts of the pinned vendor Controller and how the open implementation
follows them. DTM is a Lower Link Layer role; its implementation does not
require the vendor allocator, callback registry or RTOS. The portable
[DTM session](../../../protocols/bluetooth/le/ll/src/dtm.rs) plans the
test events, the removed radio role lowered each event into the controller-memory
graph, and supported HCI commands belong to the portable
[HCI crate](../../../protocols/bluetooth/hci/).

## Pinned public inputs

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
Role names come from the lineage and are accepted only where the pinned body
agrees in control flow and call order. The earlier archives supply no
register behavior or ABI. Object files and disassembly stay temporary review
inputs and are not repository artifacts.

## Recovered DTM component

The pinned archive keeps the DTM body in members `dtm_1.o` (HCI wrappers)
through `dtm_4.o`:

| Pinned body | Bytes | Role and behavior |
| --- | ---: | --- |
| `dtm_hci_txTest*` / `dtm_hci_rxTest*` | wrappers | Unobfuscated HCI entry points. Every transmitter variant reaches `sym_dtm_1pX1U0tSsIhRSNC4OS9q`, every receiver variant `sym_dtm_0Gv2DeOMHJNKuicdtdnN`; `dtm_hci_endTest` reaches `sym_dtm_NsbldBIeGraE2wg0AVy7`. |
| `sym_dtm_1pX1U0tSsIhRSNC4OS9q` / `sym_dtm_0Gv2DeOMHJNKuicdtdnN` | 222 / 184 | Validate and start a transmitter or receiver test: end a running test, allocate the graph and create the role context. |
| `sym_dtm_qzyGhbH1fEoUEmtalnva` | 532 | `r_ble_lll_dtm_alloc_memory`: allocate and bind the DTM memory graph (below). |
| `sym_dtm_7gOGApwmDG1TxZOfQhrh` | 288 | `r_ble_lll_dtm_reset_link_state`: reset the link-state image before every event. |
| `sym_dtm_mmDG6dYKgcDruyC7Hgxv` | 122 | `r_ble_lll_dtm_calculate_itvl`: the transmitter interval. |
| `sym_dtm_25YOj8pPkiCiy504xJbR` / `sym_dtm_uJXhYheB5ZvE92bMWBC4` | 512 / 200 | Create the transmitter or receiver context, reset the link state and build the first event. |
| `sym_dtm_2zeOUjc7g55zkDNQuZkg` | 964 | Build and publish the first event of a test. |
| `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` | 810 | Build and publish every later event. |
| `sym_dtm_ifYtgHeQ0rvhqeqBbpl1` | 370 | `r_ble_lll_dtm_recycle_sch_item`: the completion callback installed at item `+0x58`. |
| `sym_dtm_NsbldBIeGraE2wg0AVy7` | 140 | `r_ble_lll_dtm_end_test`: stop the scheduler and free the graph. |
| `sym_dtm_ikQ7xyHA5qBHWlaqAjZu` / `sym_dtm_JMPstxdIRcEVY1YJeXkA` | 72 / 28 | RX result projection and the RSSI getter behind `esp_ble_get_dtm_rx_rssi`, described with the `DtmRxResultProjection` of an implementation outside this repository. |

The CTE fields of the role context (length at `+0x1d`, type at `+0x1e`, slot
duration at `+0x1f`, switching pattern at `+0x20..`) select a separate event
branch that programs the antenna-switching engine. The open implementation
supports tests without CTE, so every event takes the no-CTE branch.

## Protocol values

The fixed `0x71764129` link-state word is retained by a private protocol-level
LE access-address type. Bluetooth Core 6.1 [Vol 6, Part F,
Section 4.1.2](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-61/out/en/low-energy-controller/direct-test-mode.html)
specifies the DTM synchronization word in transmission order; the Core
[little-endian Link Layer bit-order
rules](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-61/out/en/low-energy-controller/link-layer-specification.html)
produce the controller image `0x71764129`. The pinned legacy advertising reset
`r_sym_ble_7lsXnox2LxrGG0FmY7qR` (`r_ble_lll_adv_reset_link_state`) writes
`0x8e89bed6`, the advertising Access Address, to the same positional word.

The fixed low-24-bit `0x555555` image is retained by a private protocol-level
LE CRC-initialization type while the containing word's high byte stays
opaque. Core 6.1 [Vol 6, Part F, Section
4.1.3](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-61/out/en/low-energy-controller/direct-test-mode.html)
requires that preset for every LE test packet, and the same advertising reset
installs the same image. This names the protocol values and their SRAM codec;
it does not identify or grant ownership of the hardware consumer.

## Link-state reset

`sym_dtm_7gOGApwmDG1TxZOfQhrh` runs before every event, initial and recurring.
It installs the compressed private TX head at `+0x00` and RX tail at `+0x08`,
transforms the `+0x02` halfword, sets bits 27 and 26 of `+0x14` as the DTM
reset profile, installs the `0x555555` image at `+0x2c`, the access address at
`+0x38` and the six-bit configuration image at `+0x50`. It rewrites the low
halfword of `+0x30` as `(h & 0xc100) | 0x1e00` and stores the transmit-power
index in byte `+0x61`. A receiver stores
`r_sched_timer_convertDiffToTicks(0)`, which is zero, in `+0x34`; a
transmitter leaves that word alone. The body does not write `+0x04`.

The power index comes from `r_sym_ble_5YIHPyqYswSaQs7KMvEA`, which searches the
BLE PHY environment's provider level table for the highest level not above the
request; an implementation outside this repository carries it in `LeTxPower`
and refuses a request below the lowest level, where the vendor asserts.

The memory codec reproduces the reset as a non-publishable reviewed region.
It exposes no controller address, storage publication or hardware-ownership
transition, because the omitted descriptor words and their hardware consumer
remain open.

## Scheduler item

The first-event body `sym_dtm_2zeOUjc7g55zkDNQuZkg` writes the role byte at
`+0x02` (`0x10` transmitter, `0x40` receiver), sets bit 31 of `+0x04`, clears
bits 23:20 of `+0x08`, replicates the two-bit PHY code into bits 29:28 and
31:30 of `+0x14`, stores the seven-bit frequency image and low nibble three
in `+0x18`, writes `0xffff_ffff` to status `+0x38`, stores `0x000f0001` in
`+0x2c` for a receiver, clears the low byte of `+0x4c` and stores the raw
start and end at `+0x44/+0x48`. It then stores
`r_sched_timer_convertTimeToTicks(r_sched_timer_convertTimeToUs(+0x44))` in
`+0x0c`. It writes neither the window length at `+0x10` nor a power index
into the item.

The recurring body `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` rewrites the same
item. It stores link-state byte `+0x61` into bits 27:20 of `+0x14`, replaces
the receiver's `+0x2c` with literal one and recomputes the times and `+0x0c`
the same way. The memory codec `DtmSchedulerItemReviewedWords` of an
implementation outside this repository applies these transforms; its radio role
copies the power only for an event after the first one of a test instance.

Channel and PHY inputs are typed. `r_sym_ble_2a66ZbcncZhAIIYsMGHk`
(`r_ble_phy_chan_to_freq`) maps DTM channel `n` to frequency image `2*n`.
`r_sym_ble_xR1zQExKjmzgybrZ0t5W` (`r_ble_phy_mode_to_rate`) maps modes one,
two, three and zero to rate images zero, one, two and three. The HCI
validators map transmitter selectors one to four and receiver selectors one to
three onto those modes, so Coded S=2 exists only for a transmitter. The memory
boundary receives role-specific PHY variants and never a raw rate integer.

## Publication

The pinned DTM bodies do not use the common insertion transaction. Both
event bodies call `r_btdm_sched_stop` (`r_sym_bt_74l62ZLsZuXg67pPHSd7`), store
the item as the head of hardware list zero through
`r_btdm_sched_set_hw_list_header` (`r_sym_bt_8m3cRMNRZNfaJ7qVvayk`) and start
the scheduler with `r_btdm_sched_run` (`r_sym_bt_DPWY0umixzmXEaFuUyCI`). There is
no overlap check or lock/modify request: while a test runs, it owns list zero.
The DTM item carries allocation kind five and a private chain at link-state
`+0x64`, so the scheduler item is always the single head.

The open radio role followed that order with an explicit test mode. Opening a
test instance moved the role from shared scheduling to a test session, which
owns the Link Layer exclusively (Core Specification Vol 6, Part F):
advertising, scanning and connection requests were refused as busy until the
session had ended. A test event that found the scheduler busy returned the
`StopScheduler` step; the runtime stopped the scheduler with the common
lifecycle sequence, resumed it, and the idle scheduler then took the event as
the list-zero head and ran. An idle scheduler took it directly. The
executor's idle insertion also writes the unexecuted status and clears the
list-state bytes the executor itself uses for its mirror.

The common scheduler lifecycle, finished-list transfer and completion walk
are described in [`bluetooth-scheduler-lists.md`](bluetooth-scheduler-lists.md)
and [`bluetooth-interrupt-runtime.md`](bluetooth-interrupt-runtime.md).

## Modem ETM channels

For a test without CTE, both event bodies call
`r_sym_ble_YpJTETFGhduIAMkkBKjc` for channels zero and one. It clears the
channel's bit in the software ownership map of `r_sym_resMgmt_zyxoWB9DZg2URcqF30NE`
and writes the channel bit to `MODEM_ETM.CHANNEL_ENABLE_CLEAR`. In the vendor
Controller, channel zero carries the route that BLE PHY register
initialization `r_sym_ble_nENHlP4KBuQYlFVffaR5` (`r_ble_phy_init_registers`)
installs from modem event 8 to task 20; channel one is used only by the CTE
branch. Test End calls `r_ble_phy_init` (`r_sym_ble_r39KCENxd4X4fI1qTGp9`),
which marks the PHY for reinitialization and thereby restores the route.

This repository gives channels zero and one to IEEE 802.15.4 and runs the BLE
PHY route on channel two (see the modem ETM PAC), so it follows the vendor
semantically rather than by channel number. The first event of a test session
returns the `EnterTest` step: the runtime disables channel two through
`MODEM_ETM.CHANNEL_ENABLE_CLEAR` immediately before the list head is published
and the scheduler runs, and keeps the PAC's `BlePhyEtmRouteDisabled` owner.
The channel stays disabled for the later events of the session, as nothing
between them reinitializes the PHY. Test End returns the `LeaveTest` step,
whose restore consumes that owner and routes and enables channel two again;
a runtime fault or an uninstall restores it as well. Channel one has no counterpart because
the open implementation has no CTE route. The vendor also disables its
channels before its scheduler stop, while here the stop, when needed, happens
first; the two operations touch disjoint hardware.

## Allocation

`sym_dtm_qzyGhbH1fEoUEmtalnva` allocates, in order, the link state (kind six),
a zeroed `0x48`-byte scheduler context, the scheduler item, the bound RX
packet and header, the unbound swap header and the bound TX packet and
header. It returns `-1` for the link state and `2`, `3`, `4`, `5` for the
later stages, freeing every partial allocation. The HAL engine reserves
the graph as one non-movable static allocation retained by a movable CPU
owner and does not preserve those integers as its API.

The item receives the compressed scheduler-context link at `+0x04`, the
compressed link-state link at `+0x08`, kind byte five at `+0x4d`, the recycle
callback at `+0x58`, bit 20 in `+0x00`, `0x18000000` ORed into `+0x1c` and
four five-bit coexistence priority lanes of one each (`0x8421`) in the low
twenty bits of `+0x24`. The low twelve bits of `+0x20` are the scheduler
number derived from the configured advertising, connection and
synchronization limits. The link state receives its private anchors at
`+0x68..+0x78`: RX head/tail at `+0x68/+0x70`, TX head/tail at `+0x6c/+0x74`
and the swap reserve at `+0x78`.

The TX header allocator `r_sym_memMgmt_2LOgKQI4pa8z243VUnLR` and the TX
context body `sym_dtm_25YOj8pPkiCiy504xJbR` produce the no-CTE LE Test PDU
header at packet `+0x10`, its length at `+0x11` and payload at `+0x12`; the
binding is recorded as `BLOB_LIBBLE_DTM_PACKET_HEADER_BINDING`. Payload
selectors 1, 2, 4, 5, 6 and 7 fill `0x0f`, `0x55`, `0xff`, `0x00`, `0xf0` and
`0xaa`; selectors 0 and 3 copy PRBS9 and PRBS15, which the open implementation
regenerates with bounded LFSR steps.

## Timing

Every DTM window is planned by the portable session: a transmitter test places
its packets on the `I(L)` grid of Core 6.0 Vol 6 Part F section 4.1.6 and a
receiver listens in back-to-back windows. The vendor event bodies compute their
own anchors; the open implementation keeps the portable plan and lowers only
the resulting raw start and end.

The vendor anchor arithmetic is recorded for comparison. The first event
starts at `r_sched_timer_getCurrentTimeU32()` plus the LLL lead, 500 and the
scheduler margin `r_sym_sched_GEBmwfVsspx61ARDIjlz`, or at the RF-ready time
from `r_sym_bt_Ceh2khbCcopEBybBO6Z5` when that is later. The item starts at
the anchor minus the margin and ends at the anchor plus a duration: the
transmitter's `r_ble_ll_pdu_tx_time_get(0xff, phy)`
(`r_sym_ble_39vxJ3ouU2tgaDNBXx5H`) or the receiver's literal 1000, extended
through `r_ble_ll_usecs_to_ticks_round_up` (`r_sym_ble_BI8ShdGGOFws9OnpsN9H`).
A recurring transmitter event adds the interval to the previous anchor and
skips whole intervals it has missed; a recurring receiver event anchors at the
current time plus the margin and a configured lead plus 15, or at RF-ready.

Conversions between raw controller ticks and scheduler microseconds are
`r_sched_timer_convertTimeToUs` (`r_sym_sched_TYdJzWTeOAsagss1aIFj`) and
`r_sched_timer_convertTimeToTicks` (`r_sym_sched_sNmTBa8gkKuVLZu7GU1A`). One
standalone raw tick is half a microsecond. The radio role derived the item's
`+0x0c` through the same two projections of its retained Controller epoch.

## Completion and recycling

The finished-list broker path returns a completed item to its role callback:

```text
finished hardware-list mask
  -> brk_sym_sched_MzeSZzbQ4ZKhWW6Wu5jV = r_sched_txn_onSchedHwListDone
  -> r_sym_bt_M9nG353V0svWrv1l1zGw = r_btdm_sched_pick_finished_items
  -> software completed-item queue through item+0x54
  -> r_sym_bt_uNi9OHmE7XdXfGqTelU5 = r_btdm_recycle_in_task
  -> r_sym_bt_WHYoiw8ufY0AEM2KSRK1 = r_btdm_sched_pop_executed_sch
  -> r_sym_bt_QsLKLOCC2pct4rL8uFBN = r_btdm_recycle_process_dequeued_sch
  -> callback at item+0x58 = sym_dtm_ifYtgHeQ0rvhqeqBbpl1
```

The callback returns the item to the private chain at link-state `+0x64` and
accounts status zero only: a transmitter increments the DTM count, a receiver
drains its returned buffers through `r_sym_memMgmt_HzBmGkHAh4mavF5QJM0o` and
`r_sym_memMgmt_o8elWlcL1fxcPwWx5LoS`, rejecting a result word whose low 24
bits are nonzero and otherwise copying the RSSI byte and incrementing the
wrapping 16-bit receive count. While the test stays active, it builds the next
event through `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` whatever the status.

With capacity one, the two bound RX header slots alternate deterministically
between packet-bearing tail and packetless predecessor. The HAL engine
models that bounded rotation, exposes no raw header words and returns either
no packet or one typed result projection. The executor's completion walk
returns every executed item; the radio role then drained its receive graph and
reported one test result per receiver event before the event ends.

## Test End

`sym_dtm_NsbldBIeGraE2wg0AVy7` serializes the shared count as the Test End
result. When a test is active, it stops the scheduler with
`r_btdm_sched_stop`, unregisters the DTM event, calls `r_ble_phy_init` and
frees the graph. Cancelling a listed test event in the open radio role
returned the `StopScheduler` step instead of opening the cancellation hold;
after the stop and resume the event left its list through the idle path and
ended as not executed. A test event still waiting for insertion left at once.

There is one intentional HCI difference. The vendor callback increments the
shared count for a successful transmitter event and Test End serializes it.
Bluetooth Core 6.3 Vol 6 Part F requires the report ending a transmitter test
to contain zero
([RFPHY Test Modes](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core_v6.3/out/en/low-energy-controller/rfphy-test-modes.html)),
so the open HCI policy reports zero for a transmitter.

## Responsibility boundaries

| Owner | Contract |
| --- | --- |
| Register model and PAC | Reviewed fields, controller-time transactions, exact publication/acknowledgement and affine MMIO capabilities |
| HAL | Powered epoch, clock/reset/PHY lifecycle, finite hardware operations and interrupt routing |
| Controller memory | Pinned descriptor graphs, private pointer codecs and CPU/hardware ownership transfer |
| Scheduler executor | List-zero mirror, insertion, cancellation, completion walk and stopped state |
| Radio role | Lowering of test events, stop-then-publish for DTM and result reports |
| DTM session | Typed test parameters, event plan, Test End and counters |
| HCI adapter | Standard command/event transport and backpressure |

Sleep-enabled RF wake, CTE, general hardware-list dispatch and physical RF
qualification are separate contracts.
