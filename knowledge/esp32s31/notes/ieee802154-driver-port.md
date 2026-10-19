# ESP32-S31 IEEE 802.15.4 driver port map

This reference identifies the public ESP-IDF IEEE 802.15.4 driver that the
production crates port, pinned at commit
`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. Unlike the Wi-Fi and Bluetooth
controllers, the complete MAC driver and its register layer are public source;
only the PHY, BTBB and coexistence libraries it calls are closed. The map
assigns every source unit to its owning production layer and states the
current coverage. Coverage here is source inventory.

The [lifecycle](ieee802154-lifecycle.md), [dataplane](ieee802154-dataplane.md)
and [control](ieee802154-control.md) references record the reviewed contracts
of individual paths in detail.

Because the driver is source, ported behavior was compared with exactly these
files compiled on the host and run against recorded register-layer, platform
and callback boundaries, rather than with a captured binary. Their ESP-IDF
revision is pinned in [`vendor/artifacts.toml`](../vendor/artifacts.toml).

## Source ledger

The files were fetched from the pinned commit and hashed locally. Together they
are the complete open driver and register layer for this chip.

| Public ESP-IDF source | Lines | SHA-256 |
| --- | ---: | --- |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h) | 565 | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h) | 30 | `a66a3562ff6ef62ffa6dda90bc08e97f0d4509bcf3817cdc57795fd97daeaa30` |
| [`components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c) | 12 | `56246b6b482752e0d217e2391acb4869ae02cd18068c5ee3b361a4b7ae110995` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c) | 1258 | `9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830` |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c) | 302 | `4bc94779b0c29fdfc77dcdf0c6d3d66fad5d02324aa951d9f19877bc62532cf4` |
| [`components/ieee802154/driver/esp_ieee802154_ack.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_ack.c) | 178 | `525e1ac8b3e5bad9bfb74b7e786d72e23fd83405e3c09fbc85b8c2cb387b8a1c` |
| [`components/ieee802154/driver/esp_ieee802154_frame.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_frame.c) | 468 | `af84dc5dcc04d1ee20fd2f686ebc7220793791c7eb50bfd41107727a58d2df3d` |
| [`components/ieee802154/driver/esp_ieee802154_timer.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_timer.c) | 126 | `66a2b68bb718d8fa878cbf28488af810ecb902288fde26dc7a0e5b32dcbd1326` |
| [`components/ieee802154/driver/esp_ieee802154_event.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_event.c) | 94 | `c62a7bc108dce00e4b907c9521ad4de0c49080d6f6f5b091481f5c2f4af94063` |
| [`components/ieee802154/driver/esp_ieee802154_sec.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_sec.c) | 30 | `7f5cae45d08ec496b78fd18cfa788be8557ba91d40eb6fe77b42967961bbdc19` |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c) | 92 | `e1a012d5f359e2445128977e82a304ba94c100c2994729e062e26586596df38a` |
| [`components/ieee802154/driver/esp_ieee802154_debug.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_debug.c) | 507 | `e4dd37b1ffc462c78a12cca7d57e4e8bd4e0e8984d542012b12ce964ee9a1812` |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154.c) | 457 | `a83716d9944d4ffba1998cc64ebb635a605b60fc77c74ae6070e83a1c617f1bc` |
| [`components/ieee802154/esp_ieee802154_multipan.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154_multipan.c) | 141 | `4a82ff4a9b1eac7158ecc3c06e800ea1a844c6b2f87ff773fcd55c3cf9552aa3` |
| [`components/ieee802154/include/esp_ieee802154.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154.h) | 692 | `244f330affef9e4d4383275c678bfb2a0a5027725ffa8ebf1266897db2ca1e59` |
| [`components/ieee802154/include/esp_ieee802154_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154_types.h) | 157 | `5206f935bfaee354562e7ab87d499a196db251aa52f1d1bd9051bb32b4957424` |
| [`components/ieee802154/include/esp_ieee802154_multipan.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154_multipan.h) | 224 | `4c040a3b38769a2e697f65f21b04ea84d93f3de9048be27977b8e61b55777146` |
| [`components/ieee802154/private_include/esp_ieee802154_dev.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_dev.h) | 275 | `afdb884e6dc78f19c9d72adedfc1ea7fdbbdebb86d1e7aaa80bb89e62a74bfd3` |
| [`components/ieee802154/private_include/esp_ieee802154_pib.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_pib.h) | 281 | `cdad2d0fa07babe25f0174d69d551ca7acb6bd250ff9614961d0bff55638d835` |
| [`components/ieee802154/private_include/esp_ieee802154_frame.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_frame.h) | 228 | `c4d326c59bd71a43db2de265ab3886064faf95ee018a541fc2deb0bc5609d1e1` |
| [`components/ieee802154/private_include/esp_ieee802154_ack.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_ack.h) | 82 | `977df90ca0c0db4f2148c66d8956c49bdfd7418e7a33925b6495144be6db197d` |
| [`components/ieee802154/private_include/esp_ieee802154_timer.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_timer.h) | 156 | `e5ab8062c0c4b9195bd597a79a6aabb692fdb686a9599a034db36a46d8f89bd6` |
| [`components/ieee802154/private_include/esp_ieee802154_util.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_util.h) | 467 | `4ca86544b16248e1d66b85cc84df8ac37bde50727189b3b242513aab22e19017` |

## Interrupt-context decisions

The driver takes every time-critical decision inside the interrupt handler
([`ieee802154_isr`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L782-L895)),
not in a task:

- `RX_DONE` selects the ACK path before the acknowledgement leaves: it
  computes the frame-pending bit from the pending-address table
  (`ieee802154_ack_config_pending_bit`), and for a 2015 frame generates the
  enhanced ACK, publishes its address and notifies the hardware
  ([L538-L585](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L538-L585));
- `TX_DONE` of an ACK-requesting frame enters ACK reception and starts the
  timer-zero watchdog
  ([L507-L536](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L507-L536));
- a receive abort (CRC, filter, SFD timeout, ...) is not reported upward; the
  deferred `next_operation` re-enables receive when `rx_when_idle` is set, or
  enters idle/sleep
  ([L488-L505](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L488-L505),
  [L604-L640](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L604-L640)).

Task-side entry points (`ieee802154_transmit`, `receive`, `energy_detect`,
`cca`, `sleep`) run under the same critical section and always pass through
`stop_current_operation`, which applies the stop path of the current private
state
([L412-L461](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L412-L461)).
Consequently the owner of the MAC state machine must be reachable from both
interrupt and task context; an interrupt that only acknowledges events and
defers every decision to a task cannot meet these deadlines.

## Closed dependencies

| ESP-IDF call | Purpose in the driver | Open counterpart in this repository |
| --- | --- | --- |
| `esp_phy_enable` / `esp_phy_disable(PHY_MODEM_IEEE802154)` | RF client acquire and release around operations | PHY `join_ieee802154` / `leave_ieee802154` on the arbiter's shared PHY domain |
| `esp_btbb_enable` / `esp_btbb_disable` | common BTBB initialization | arbiter BTBB reference, taken by `join_ieee802154` with the registration's gain byte |
| `bt_bb_get_tx_pwr_table` | dBm-to-power-index table | HAL `ESP32S31_TX_POWER_LEVELS`, recovered from the provider |
| `esp_coex_ieee802154_*` | PTI scenes, external-coexistence stages, coexistence break notice | PTI levels: HAL scene priorities read from the arbiter table, published by the driver engine. The external-coexistence stages are a bare `ret` on the pinned ESP32-S31 libcoexist; `esp_coex_ieee802154_force_rx_enable`, which external-coexistence builds call at MAC init and deinit, writes `FORCE_RX_ENB` and `COEX_ARB_DELAY` ([lifecycle](ieee802154-lifecycle.md)) and is absent with the external coexistence it serves. The break notice has no effect on the pinned ESP32-S31 libcoexist (`esp_coex_ieee802154_coex_break_notify` tail-calls `hal_ieee802154_coex_break_notify`, a bare `ret`; reviewed fact `BLOB_LIBCOEX_IEEE802154_HANDOFF`), so the engine sends none; the provenance check follows that body |
| `ieee802154_txon_delay_set` (called by `ieee802154_mac_init`) | MAC TX/RX timing delays | `TXON_DELAY`, `RXON_DELAY`, `TXRX_SWITCH_DELAY` and `TXOFF_DELAY` in the HAL MAC foundation; the shared auxiliary TX-on delay keeps the BTBB initialization value |
| `bt_bb_get_cur_rx_info` | receive diagnostic | the PAC's `Ieee802154BasebandRxInfo` capability reads its RSSI byte; the rest of the word stays unmodelled |
| `modem_clock_module_*` | module clock, reset and MAC reset | arbiter modem clock planner; MAC reset in the HAL IEEE 802.15.4 lifecycle |
| `sleep_retention_*`, `esp_phy_modem_init` | register retention across light sleep | none; the vendor module graph and the gates that hold it are in the [lifecycle reference](ieee802154-lifecycle.md#sleep-retention-and-rf-gating) |

## Ownership map

`Implemented` means production source owns the complete unit, `partial` that a
subset exists, and `absent` that no production owner exists. The owner column
is the layer that holds the unit under the PAC/HAL rule: the PAC performs
single register transactions, the HAL owns multi-step hardware sequences and
route state, the driver owns the IEEE 802.15.4 MAC state machine, and portable
frame semantics belong to the protocol crate.

| ESP-IDF unit | Owner | Coverage |
| --- | --- | --- |
| `ieee802154_common_ll.h`: command, event, abort, address, policy, ED, timer, security, pending, status, enhanced-ACK and diagnostic-counter accessors | PAC `ieee802154` | implemented: one typed transaction per accessor on the task register lease, alongside the composite configuration transactions built from the same steps. The diagnostic counters read the sixteen-bit field of `ieee802154_reg.h`; the public struct reads the complete word of the single-counter registers |
| `ieee802154_ll.h` (S31): `IEEE802154_RSSI_COMPENSATION_VALUE` 0, receive sensitivity -104 | driver engine | implemented: RSSI compensation 0; `RECEIVE_SENSITIVITY_DBM` -104, which the engine's `capabilities` publishes in the port's `Ieee802154Capabilities`; this repository has no OpenThread adapter that reports it |
| `ieee802154_mac_init` / `deinit`, `ieee802154_enable` / `disable` | HAL IEEE 802.15.4 lifecycle, PHY `ieee802154_client`, HAL `ll`, driver engine | partial: arbiter power and module clocks, PHY client and BTBB, MAC reset and masked foundation with the `ieee802154_txon_delay_set` delays; the engine's `mac_init` reinitializes the PIB, applies `ll::mac_init_registers` and clears the receive ring; the operational owners return through a foundation readback. `deinit` writes no MAC register. Interrupt allocation: `esp_intr_alloc(132, 0)` comes last in `mac_init` and `esp_intr_free` first in `mac_deinit`, before BTBB, the PHY client and the modem clock are released; flags 0 select the lowest free level of 1 to 3 (`ESP_INTR_FLAG_LOWMED`, `intr_alloc.c` `get_available_int`), a level-triggered, non-IRAM vector on the calling core. This repository has no radio system that binds source 132 through the esp-hal route adapter at `Priority1` on the calling core after the runtime is installed and quiesces it first on stop; the host comparison pinned the vendor order and a hardware route probe pinned the level retrigger |
| `ieee802154_pib_*`, `ieee802154_pib_update` | HAL `pib` | implemented: mutable PIB with pending mark, per-channel power resolved through the external level provider, pending mode and `rx_when_idle`; the engine publishes it before each operation |
| `start_ed`, `tx_init`, `rx_init`, `ieee802154_transmit`, `receive`, `energy_detect`, `cca`, `sleep` | driver engine, runtime entry points | implemented |
| `event_end_process` register steps | HAL `ll` | implemented |
| `stop_rx` ... `stop_ed`, `stop_current_operation` | driver engine over HAL `ll` | implemented |
| `ieee802154_isr`, `isr_handle_*`, `next_operation`, private 12-state machine | driver engine | implemented without test mode; software coexistence and multi-PAN are construction options |
| RX buffer ring, `set_next_rx_buffer`, frame info | driver engine | implemented: twenty engine-owned receive buffers, the stub buffer, a copied transmit frame and the enhanced-ACK frame; DMA-capable placement is the integrating firmware's obligation |
| `esp_ieee802154_timer.c` | HAL `ll` timers, driver engine callbacks | implemented |
| `ieee802154_transmit_at`, `receive_at` (timer and ETM) | driver engine over HAL `ll` | implemented on the IEEE 802.15.4 channels zero and one of `MODEM_ETM`; the write-trigger semantics of the set and clear words is not yet verified on hardware |
| `esp_ieee802154_ack.c` pending table and `ack_config_pending_bit` | protocol pending table, driver engine | implemented for interface zero |
| `esp_ieee802154_frame.c` | protocol `mac::header` | implemented over `[PHR, PSDU...]` images; malformed headers report absent fields where the vendor reads outside the frame |
| `esp_ieee802154_sec.c` | driver engine over HAL `ll` | implemented |
| `esp_ieee802154_multipan.c`, `update_mpf_index`, per-interface pending tables | driver engine | implemented for one to four interfaces in the engine; the radio role that keeps security per interface as the multi-instance OpenThread port does, and the runtime that parks a multi-PAN engine, are not in this repository |
| `esp_ieee802154_event.c` callbacks | driver engine environment | implemented; the engine arms transmit security for a secured ACK over the portable formatter; the runtime event queue and the radio role's port of the OpenThread enhanced-ACK generator are not in this repository |
| `esp_ieee802154_util.c` coexistence scenes, channel conversion | HAL, driver engine | implemented: scene levels with the default configuration, channel conversion |
| `ieee802154_sleep`, `rf_enable` / `rf_disable`, sleep retention | driver engine, HAL and PHY | partial: engine sleep state; RF gating (off in the default build) and retention absent |
| `esp_ieee802154_debug.c` | driver engine | partial: the TX/RX statistics (`CONFIG_IEEE802154_TXRX_STATISTIC`) in the engine; no runtime in this repository collects them on request; its record and assert debugging is absent |
| `esp_ieee802154.c` public API | driver engine | partial: PIB, identity, ACK-timeout, transmit-security, pending-table and operation entry points of interface zero; event callbacks are the engine environment; TX/RX statistics and the recent LQI in the engine. The radio role over the portable contract and the holder of the coexistence configuration (`update_coexistence`, `coex_config`) are not in this repository. `esp_ieee802154_get_recent_rssi` (and the OpenThread port's `otPlatRadioGetRssi`) is the HAL's `recent_rssi`, a live read of the low byte of the baseband receive information (`bt_bb_get_cur_rx_info`, BTBB `CURRENT_RX_INFO`) through a read-only capability carved out of the shared baseband; an OpenThread adapter outside this repository answers `otPlatRadioGetRssi` with it |

The masked foundation with serialized polled ED and CCA has no ESP-IDF
counterpart. It is retained as a hardware diagnostic path only.
