# ESP32-S31 IEEE 802.15.4 control-state contracts

This reference describes public ESP-IDF source contracts pinned at commit
`4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`. Source order, software state and
hardware readiness are distinct claims. Reviewed register semantics belong to
[the register model](../registers/README.md); production
implementation retains its own authority.

## Source ledger

Hashes identify the complete pinned public files.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/ieee802154/private_include/esp_ieee802154_dev.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/private_include/esp_ieee802154_dev.h#L27) | exact private 12-state enum | `5a8d7d8db146a78bb8a7281c098006375c3708b1523b8541b8e50fbee31279c5` |
| [`components/ieee802154/include/esp_ieee802154_types.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/include/esp_ieee802154_types.h#L19) | five public radio states | `5206f935bfaee354562e7ab87d499a196db251aa52f1d1bd9051bb32b4957424` |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/esp_ieee802154.c#L287) | private-to-public state projection | `bfa12943c503eb9c56e7774faccb9429d6f26e02db0b82afb2c7fdfe1752fd7a` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | deferred-next flag and timer-zero callback | `517e0c761e5f01b3bf1797bbbbc54a16d2a9e4b79d8e32a345124d9bc6e4b9e4` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | state-specific stop and next-operation paths | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | event-handler state changes and terminal effects | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | snapshot, acknowledgement, assertions, and ordered dispatch | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | operation-start order and timed receive callbacks | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1240) | RF client guard, sleep, ED, and CCA entry | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_timer.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_timer.c#L13) | timer callback storage, clearing, and invocation | `66a2b68bb718d8fa878cbf28488af810ecb902288fde26dc7a0e5b32dcbd1326` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L175) | opcodes and named event bits | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L175) | command, event, abort, and direct-address accessors | same file/hash as above |
| [`components/soc/esp32s31/register/soc/ieee802154_reg.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/register/soc/ieee802154_reg.h#L184) | event status without an access-class annotation | `fd3f944ac97634605083031f96c0f942af26a81a9e9a3123281c59e5719f9d9c` |
| [`components/soc/esp32s31/register/soc/ieee802154_struct.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/register/soc/ieee802154_struct.h#L143) | event-status bitfield without acknowledgement semantics | `da13c2bc78cd6ef35a4e54ddddf11ce48fda967746193f1a0ad03578a5881752` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_phy/src/phy_init.c#L282) and [L997-L1023](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_phy/src/phy_init.c#L282) | shared PHY client lifecycle and opaque registration/calibration | `e50b07f5e9c2fcbf4bbf3ad0506a94ebf15dd9fc515a2cf0434bcb4d18ae768a` |
| [`components/esp_phy/include/esp_private/phy.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_phy/include/esp_private/phy.h#L38) | opaque PHY wakeup and PLL declarations | `6cd2bded3218cd80c485677358aaba351f56d53d915be87f77faa5dcf6d025cb` |
| [`components/esp_phy/src/btbb_init.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_phy/src/btbb_init.c#L95) | BTBB first-user initialization and refcount | `51f669948ed02e84531dc7edbbe1ddc9e46920c2ad5021700d4464aab5981ba3` |
| [`components/esp_phy/include/esp_private/btbb.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_phy/include/esp_private/btbb.h#L14) | opaque BTBB initializer declaration | `659a94ca15d9e7d5531f34c755476523d285bc2cbc0a378b58505e67facea289` |

## 1. Exact software states and public projection

The private driver enum has twelve implicit, consecutive C values. Order is
part of the source observation; it is not a claim about a hardware-state field.
The public getter collapses those twelve values into five API states exactly as
shown:

| Value | Private state | Public API state |
| ---: | --- | --- |
| 0 | `IEEE802154_STATE_DISABLE` | `ESP_IEEE802154_RADIO_DISABLE` |
| 1 | `IEEE802154_STATE_IDLE` | `ESP_IEEE802154_RADIO_IDLE` |
| 2 | `IEEE802154_STATE_SLEEP` | `ESP_IEEE802154_RADIO_SLEEP` |
| 3 | `IEEE802154_STATE_RX` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 4 | `IEEE802154_STATE_TX_ACK` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 5 | `IEEE802154_STATE_TX_ENH_ACK` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 6 | `IEEE802154_STATE_TX_CCA` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 7 | `IEEE802154_STATE_TX` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 8 | `IEEE802154_STATE_TEST_TX` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 9 | `IEEE802154_STATE_RX_ACK` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 10 | `IEEE802154_STATE_ED` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 11 | `IEEE802154_STATE_CCA` | `ESP_IEEE802154_RADIO_TRANSMIT` |

The projection is deliberately counterintuitive in two places: standalone ED
is publicly receive, while standalone CCA and ACK reception after transmit are
publicly transmit. A five-state public value cannot recover the private phase
and is not sufficient to select a stop path or validate an interrupt batch.

The private value is software-owned. For example, timed receive assigns
`RX` before ETM starts the hardware receiver, and transmit assigns `TX` or
`TX_CCA` immediately after issuing the corresponding command. This reference does
not equate that enum with instantaneous RF or MAC hardware state.

## 2. ISR legality is sequential, not set-wise

The ISR enters the driver spinlock, samples the event image plus both abort
reasons, passes the complete snapshot to the LL helper named `clear_events`,
and then visits the local snapshot in this exact order. The target reviewed fact
assigns this register W1C semantics. The generated PAC mirrors the source
transaction with a non-replayable snapshot: the hard-IRQ owner samples the
complete event field and selected abort/ED sidebands, then consumes that exact
snapshot during acknowledgement. Same-bit arrival and level-line retrigger
do not change that access model.
A hardware route probe answered both on the ESP32-S31 over the production
owners and route adapter: a second arrival of an event whose bit is still
latched is cleared with the first (the arrivals coalesce, as the
vendor's snapshot-then-clear ISR also assumes), and an event latched after the
ISR's snapshot retriggers the level-driven source-132 route once the snapshot
is consumed.

1. RX-abort phase one;
2. RX SFD;
3. TX SFD;
4. TX done;
5. RX done;
6. ACK TX done;
7. ACK RX done;
8. RX-abort phase two;
9. TX abort;
10. ED done;
11. timer zero;
12. timer one;
13. at most one deferred `next_operation()` call.

The explicit state assertions at each dispatch point are:

| Event | State accepted at that point in the ordered dispatch |
| --- | --- |
| `RX_SFD_DONE` | `RX`, `RX_ACK`, `TX`, `TX_CCA`, or `TX_ENH_ACK` |
| `TX_SFD_DONE` | `TX`, `TX_CCA`, `TEST_TX`, `TX_ENH_ACK`, or `TX_ACK` |
| `TX_DONE` | `TX`, `TX_CCA`, or `TEST_TX` |
| `RX_DONE` | `RX` |
| `ACK_TX_DONE` | `TX_ACK`, `RX`, or `TX_ENH_ACK` |
| `ACK_RX_DONE` | `RX_ACK`, `TX`, `TX_CCA`, or `TX_ENH_ACK` |
| `ED_DONE` | `ED` or `CCA` |
| `TIMER0_OVERFLOW` | `RX_ACK`, except for the separate test-timer allowance |
| `TIMER1_OVERFLOW` | no state assertion; behavior depends on the installed callback |

Abort legality is reason-dependent rather than one event-wide state set:

- RX `SFD_TIMEOUT`, `CRC_ERROR`, `INVALID_LEN`, `FILTER_FAIL`, `NO_RSS`,
  `UNEXPECTED_ACK`, `RX_RESTART`, and `COEX_BREAK` assert `RX` in phase one.
  `ED_ABORT` and `ED_COEX_REJECT` assert `ED` or `CCA`. `RX_STOP`,
  `TX_ACK_STOP`, and `ED_STOP` return without a state assertion.
  `TX_ACK_TIMEOUT`, `TX_ACK_COEX_BREAK`, and `ENHACK_SECURITY_ERROR` are
  deferred to phase two, where the `TX_ACK`/`TX_ENH_ACK` assertions are
  conditional debug-monitor assertions.
- TX `RX_ACK_SFD_TIMEOUT`, `RX_ACK_CRC_ERROR`, `RX_ACK_INVALID_LEN`,
  `RX_ACK_FILTER_FAIL`, `RX_ACK_NO_RSS`, `RX_ACK_COEX_BREAK`,
  `RX_ACK_TYPE_NOT_ACK`, `RX_ACK_RESTART`, and `RX_ACK_TIMEOUT` assert
  `RX_ACK`. `TX_COEX_BREAK` and `TX_SECURITY_ERROR` assert `TX` or `TX_CCA`;
  `CCA_FAILED` and `CCA_BUSY` assert `TX_CCA`. `RX_ACK_STOP` and `TX_STOP` do
  not establish a normal-build state assertion.

These assertions observe the current mutable software state after all earlier
handlers, not a state frozen beside the event snapshot. Earlier handlers also
run callbacks and stop timers. Two source-backed same-snapshot examples show
why a set-wise validator is insufficient:

- `TX_DONE` for a frame requiring an ACK changes `TX`/`TX_CCA` to `RX_ACK` and
  assigns deferred-next false. A later co-latched `ACK_RX_DONE` is then legal,
  completes the logical transmit, and assigns deferred-next true.
- `RX_DONE` may change `RX` to `TX_ACK` or `TX_ENH_ACK` and assign
  deferred-next false. A later co-latched `ACK_TX_DONE` accepts that new state,
  delivers the received frame, and assigns deferred-next true.

Conversely, `event_end_process()` in an earlier handler stops both timers and
clears their callback slots. A co-latched Timer1 bit is still visited later,
but its handler can then find no callback. A faithful vendor comparison must
therefore exercise compiled production control logic in order; it cannot call
independent shadow handlers or validate every event against only the entry
state.

## 3. Deferred-next is last-assignment-wins

`NEEDS_NEXT_OPT(a)` is a plain assignment to one global boolean. It is not an
OR, a counter, or an append-only request. Ordered handlers can assign it more
than once, and the last executed assignment determines the end-of-batch
decision. In particular, a transition into ACK TX/RX assigns false so the
operation remains active, while a later ACK completion assigns true. Some
TX-abort branches also assign false, so replacing the fold with `any(true)`
would change behavior.

After all recognized bits have been visited, the ISR checks the final boolean.
If true, it calls `next_operation()` exactly once and then clears the boolean.
`next_operation()` first resolves a pending timed-receive stop, then either
starts another receive when `rx_when_idle` is set or moves through the
idle/sleep policy. It is not called once per terminal bit.

This proves an ordered, single-decision policy for accepted source paths. It
does not prove that every arbitrary multi-bit combination is valid. The vendor
ISR may already have performed earlier callbacks before a later assertion
detects an impossible combination; a safe pure model may instead reject the
whole unsupported batch before exposing any effect. Such fail-closed behavior
is intentionally narrower and must not be labelled exact vendor equivalence.

## 4. `STOP` is a state-specific reconciliation request

`stop_current_operation()` first clears both ETM channels and stops both MAC
timers. It then selects a private-state-specific path:

| Private state | Source action after common timer/ETM teardown |
| --- | --- |
| `DISABLE` | no command and no event reconciliation |
| `IDLE` | issue `STOP`; no status sample |
| `SLEEP` | no command and no event reconciliation |
| `RX` | issue `STOP`; sample events once; deliver an already-latched `RX_DONE`; clear `RX_DONE`, `RX_ABORT`, and `RX_SFD_DONE` |
| `TX_ACK` | issue `STOP`; clear security; synthesize receive delivery; clear `ACK_TX_DONE`, `RX_ABORT`, and `TX_SFD_DONE` |
| `TX_CCA` | run the TX stop path, then additionally clear `TX_ABORT` |
| `CCA` | issue `STOP`; clear `ED_DONE` and `RX_ABORT` |
| `TX` | issue `STOP`; sample events once; report done only for an already-latched eligible `TX_DONE`, otherwise report abort; clear `TX_DONE`, `TX_ABORT`, and `TX_SFD_DONE` |
| `TX_ENH_ACK` | use the TX stop path, but synthesize receive delivery and additionally clear `ACK_TX_DONE` |
| `TEST_TX` | no switch arm; reaches the default assertion |
| `RX_ACK` | issue `STOP`; sample events once; stop/mask timer zero; report transmit done for an already-latched `ACK_RX_DONE`, otherwise no-ACK failure; clear `ACK_RX_DONE`, `RX_SFD_DONE`, and `TX_ABORT` |
| `ED` | issue `STOP`; clear `ED_DONE` and `RX_ABORT` |

The function does not set a common post-stop state. Its boolean return is
always true and callers do not use it as a completion predicate. Where an
event sample exists, it is one immediate read after issuing opcode `0x45`;
there is no command-busy check, completion bit, fence, delay, or bounded poll.
Several paths only issue the command and clear selected event bits.

Operational start paths then continue immediately. For example, TX setup calls
stop, refreshes the PIB, publishes TX and optional ACK-RX addresses, and issues
`TX_START` or `CCA_TX_START`. Source order alone therefore does not prove that
the old engine has stopped touching DMA before a new address is published. Nor
does it prove that the event sample after `STOP` contains every terminal edge
that races with the request.

The reviewed target fact assigns W1C, the SVD publishes `oneToClear`, and the PAC
does not expose an ordinary RW modify operation or a caller-provided clear
mask. Both the cold/polled path and the disjoint hard-IRQ owner acknowledge by
consuming the exact complete snapshot they just sampled. This fixes the
software access model, but does not prove that `STOP` is synchronous or that DMA
is quiescent after the immediate source sequence. A same-bit arrival during
the acknowledgement window is lost, which the route probe observed on hardware.

## 5. Timer1 callback context and the unhandled clock event

The Timer1 implementation stores one function pointer and one opaque context.
On `TIMER1_OVERFLOW`, while the MAC ISR still holds its spinlock, the handler:

1. copies the callback and context to locals;
2. clears both global slots;
3. invokes the callback inline in interrupt context;
4. returns to the ISR, which only then evaluates deferred-next.

Clearing before invocation explicitly permits the callback to install a new
timer callback. There is no Timer1 state assertion. Timed receive uses this
facility as a two-stage chain: the first callback schedules the receive-window
end, and the second either leaves an in-progress frame running with a pending
stop or performs the state-specific stop and reports the window complete.
These callback, lock, ETM, SFD, and stop races are outside the untimed pure
control subset.

`IEEE802154_EVENT_CLOCK_COUNT_MATCH` is named as bit 10. Initialization's
non-test event image `0x3eff` leaves that bit enabled, but the reviewed ISR has
no branch for it. The bit remains in the local `events` value and reaches the
final `events == 0` assertion. Unnamed mask bits 7 and 13 have the same lack of
dispatch semantics. Until their purpose and acknowledgement behavior are
proved, a safe event batch must reject them before any modeled transition and
an active IRQ mask must not enable them.

## 6. RF, BTBB, and PLL remain outside the control proof

Every ordinary RX, TX, CCA, and ED entry first passes through the RF-enable
guard. The public enable/lifecycle path also acquires shared modem-clock and PHY
clients and enables BTBB before MAC operation. Public source exposes the
ownership shape, but not enough implementation to reproduce the physical
postconditions:

- first-user BTBB initialization calls opaque `bt_bb_v2_init_cmplx(1)`;
- PHY registration/calibration reaches opaque `register_chipv7_phy(...)`;
- RF wakeup and tracking depend on opaque `phy_wakeup_init()`,
  `phy_track_pll_init()`, and `phy_track_pll()` effects;
- MAC initialization calls closed `ieee802154_txon_delay_set()`, whose
  recovered effects are the MAC `TXON_DELAY`, `RXON_DELAY`,
  `TXRX_SWITCH_DELAY` and `TXOFF_DELAY` fields;
- TX policy uses the level set of closed `bt_bb_get_tx_pwr_table()`, recovered
  as `ESP32S31_TX_POWER_LEVELS`.

The PHY client set is shared with Wi-Fi and Bluetooth, and BTBB has its own
first/last-user refcount. A control owner cannot safely replace either with an
unconditional boolean enable or release. Clock acquisition and MAC foundation
writes do not imply calibrated RF, a locked/tracked PLL, valid TX timing, or
on-air capability.

## Implementation boundary

The [MAC engine](../../espressif/ieee802154-engine/src/engine.rs)
owns the vendor state machine, `next_operation` and its DMA buffers; the
HAL's MAC owners hold the PAC's generated register parts. This repository has no runtime that serializes the
engine with the MAC owners and routes source 132.
A `STOP` write is not a proof of terminal DMA quiescence. RF/BTBB readiness,
TX-power policy and on-air service require separate evidence.
