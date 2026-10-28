# ESP32-S31 IEEE 802.15.4 host stand

This workspace runs the public ESP-IDF IEEE 802.15.4 driver on the host so the
production port can be compared with the real vendor behavior. Unlike the PHY,
Wi-Fi and Bluetooth libraries, this driver is published as source, so the stand
compiles it directly instead of executing a captured binary. The
[MAC engine](../../../chips/espressif/ieee802154-engine/README.md) identifies the pinned
sources and their production owners.

## What is compiled and what is recorded

The chip's [`artifacts.toml`](../../../chips/esp32s31/vendor/artifacts.toml) pins every translation unit and
every ESP-IDF header the build uses, with its SHA-256 at the pinned revision.
[`build.rs`](build.rs) refuses to build unless each file matches. The driver
sources are compiled unmodified with the Kconfig defaults in
[`shim/include/sdkconfig.h`](shim/include/sdkconfig.h).

The shared ESP-IDF ledger also pins platform sources for analysis. The stand
compiles only `components/ieee802154/*.c` (including its `driver/` sources)
and the S31 `ieee802154_periph.c` connection used for modem clocks and
interrupt allocation. It does not compile the SPI flash HAL: the driver and
its recording boundaries reference no `spi_flash_hal_*` symbols.

The stand replaces only the boundaries the driver does not own:

| Boundary | Replacement |
| --- | --- |
| Every `ieee802154_ll_*` register accessor | Generated from the real `ieee802154_common_ll.h`: the vendor types and constants stay, each accessor records its name and arguments |
| Direct `REG_READ`/`REG_WRITE` (the ETM helpers) | Recorded register reads and writes |
| PHY, BTBB, coexistence, modem clocks, `ieee802154_txon_delay_set`, `bt_bb_get_cur_rx_info` | Recorded calls ([`shim/src/host.c`](shim/src/host.c)) |
| `bt_bb_get_tx_pwr_table` | The recovered ESP32-S31 level set ([`src/record.rs`](src/record.rs)) |
| `esp_intr_alloc` | Records the allocation and captures the handler that scenarios invoke |
| `esp_timer_get_time`, the driver spinlock, `assert` | Recorded; a failed assertion becomes a trace record |
| Application callbacks (`esp_ieee802154_receive_done`, ...) | Recorded with their frame bytes and frame information |

Register-layer getters answer from scenario inputs, or with the last value
the matching setter wrote. The model supplies nothing else: event images,
abort reasons and received frames are explicit scenario steps.

## Obtaining the sources

```console
cargo pins fetch --chip esp32s31
```

downloads every pinned ESP-IDF file of source `esp-idf` into
`target/vendor/esp-idf/<revision>/`, which keeps the checkout's relative
layout, and verifies it. The build reads that directory; `OER_ESP_IDF_DIR`
may name a full checkout at the pinned revision instead. Either way every
file must match its pinned SHA-256. The stand needs a host C compiler.

## Running

```console
cargo test -p oer-vendor-ieee802154-esp32s31 -p oer-tool-vendor-ieee802154-esp32s31-cli
cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- list
cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- run transmit-with-ack
cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- compare transmit-with-ack
cargo run -p oer-tool-vendor-ieee802154-esp32s31-cli -- compare all
```

The library here decides the verdicts; its program,
`tools/vendor/ieee802154-esp32s31-cli`, runs them. `compare all` compares every
catalog scenario, each in its own process, and fails at the first failed
comparison. A single `compare` exits 0 for MATCH, 1 for DIFF and 2 for
INCOMPLETE.

The Cargo integration test `shim_interrupts` compiles the actual `shim/src/host.c`
with an isolated C recorder and runs it in its own process. It checks distinct
interrupt handles, scalar argument recording and positive/negative return
values for `esp_intr_disable` and `esp_intr_enable`. Compiler outputs stay in
the test's own directory under `target/`.

The compiled driver keeps its state in C globals, so each process runs one
scenario. `run` prints the scenario's boundary trace, one record per line;
buffer addresses appear as `tx#n` (the `n`th transmitted frame) and `buf#n`
(the `n`th distinct driver buffer). The [scenario catalog tests](src/catalog.rs)
check the catalog against expectations read from the pinned source.

## Comparing the production engine

`compare` runs the same scenario through the production engine
(`oer_espressif_ieee802154_engine::engine`) and prints `MATCH`, `DIFF` with the first
differing record, or `INCOMPLETE` with the reason. [`src/port.rs`](src/port.rs)
renders each HAL low-level call as the vendor `ieee802154_ll_*` accessor and
argument, and each modem ETM access as the vendor's direct register access;
getters on both sides answer from one shared value model. Calls that leave
the driver (clocks, PHY, BTBB, interrupt allocation, time, critical sections)
are not compared. A vendor assertion, or a step the engine does not own, is
`INCOMPLETE`. Address, extended-address and key arguments are compared by
content. Both sides resolve transmit power against the ESP32-S31 BTBB level
set recovered from the vendor library (`TX_POWER_LEVELS_DBM`): the stand
provides it as the driver's `bt_bb_get_tx_pwr_table`, and the engine uses the
a typed level set composed from chip knowledge's `TX_POWER_LEVELS_DBM`
(`oer-esp32s31-knowledge`); the stand links no HAL. The catalog requires `MATCH` or its exact documented `EXPECTED` trace pair.

The `recent-rssi` scenario reads `esp_ieee802154_get_recent_rssi` disabled,
idle, receiving, awaiting an ACK and asleep, over receive-information images
with nonzero upper bits. The production read is the runtime's over the PAC's
baseband capability and passes no engine or MAC accessor, so the port
records nothing for it: `MATCH` means the vendor call leaves no
register-layer access in any state either. The value is not compared; a test
pins it to the signed low byte of one `bt_bb_get_cur_rx_info` call, the
geometry the register model publishes for the PAC read.

## ACK timer comparison expectations

Each row keeps its complete vendor fingerprint. The production trace also
retains configuration at initialization (`mac_init` snapshots/republication),
where ESP-IDF `4d80c1b2` [`esp_ieee802154_dev.c:956::ieee802154_mac_init`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L956) calls
[`esp_ieee802154_pib.c:56::ieee802154_pib_init`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_pib.c#L56) to restore PIB defaults.
ACK timer differences follow [the affine acknowledgement contract in #693](https://github.com/ermacv/open-esp-radio/pull/693).
Its hardware source is `HW8_S31_IEEE_MASKED_STATUS_20261026`, run
`hw8-20261026-radio-local-05`, `ieee-timers.json`, retained-snapshot and
selective-W1C phases, [PR #689](https://github.com/ermacv/open-esp-radio/pull/689).
Stale timer0 is cleared while stopped before arming; completion/stop
acknowledges the enabled timer before masking. No frame or callback is lost.

| Scenario | Source and complete-trace distinction |
| --- | --- |
| `recent-rssi` | [`esp_ieee802154_dev.c:431::stop_current_operation`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L431) → [`esp_ieee802154_dev.c:400::stop_rx_ack`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L400), [`esp_ieee802154_timer.c:23::ieee802154_timer0_stop`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_timer.c#L23): production stops before sampling and includes timer0 in the saved acknowledgement; C masks then clears the non-timer event set. RSSI calls add no LL operations. |
| `transmit-abort-reasons` | [`esp_ieee802154_dev.c:400::stop_rx_ack`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L400) between attempts; [`esp_ieee802154_dev.c:526::isr_handle_tx_done`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L526) starts each ACK wait. Production consumes stale timer0 before arming and the saved enabled timer0 on stop. All abort outcomes remain equal. |
| `transmit-with-ack` | [`esp_ieee802154_dev.c:526::isr_handle_tx_done`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L526), [`esp_ieee802154_dev.c:614::isr_handle_ack_rx_done`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L614): production clears stale timer0 before arming, then acknowledges timer0 before masking on ACK completion. The received ACK and completion metadata remain equal. |
| `transmit-ack-timer-expires` | [`esp_ieee802154_dev.c:175::receive_ack_timeout_timer_start`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L175), [`esp_ieee802154_dev.c:526::isr_handle_tx_done`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L526), [`esp_ieee802154_timer.c:90::ieee802154_timer0_fire_at_with_callback`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_timer.c#L90), [`esp_ieee802154_timer.c:102::isr_handle_timer0_done`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_timer.c#L102): the extra acknowledgement clears stale timer0 before arming; expiry consumes the sampled overflow and reports the same missing ACK. |
| `transmit-ack-abort-timeout` | [`esp_ieee802154_dev.c:741::isr_handle_tx_abort`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L741) (`RX_ACK_TIMEOUT` at [dev.c:770](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L770)): production acknowledges timer0 while enabled before masking it, after the same abort-event acknowledgement; the missing-ACK callback remains equal. |

`malformed-security`, `truncated-security-header` and `malformed-security-ie`
isolate rejection from initialization: each requires invalid-argument return
and no security setters, with no accepted fingerprint difference.

Malformed secured output from the enhanced-ACK generator follows
[`esp_ieee802154_dev.c:583–596`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L583):
STOP and receive completion without an ACK. The environment receives an
`Ieee802154SecurityFrameError` diagnostic; the engine publishes no security
setter or TX DMA address and returns no fault. No retained record is involved.

A direct security-disabled configuration violates the vendor assertion at
[`esp_ieee802154_sec.c:19`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_sec.c#L19),
so comparison is INCOMPLETE. Only malformed geometry returns
`ESP_ERR_INVALID_ARG` (258) at that file's line 24. Vendor assertions take
precedence over external-call and return-value comparison obligations.

## Multi-PAN

`--features multipan` builds the vendor driver with
`CONFIG_IEEE802154_MULTI_PAN_ENABLE` and the Kconfig default of two
interfaces, compiles `esp_ieee802154_multipan.c`, constructs the port with the
same interfaces and adds the multi-PAN scenarios. Every scenario is compared
in both builds; the two expectation tests that pin the default build's trace
run only without the feature.

```console
cargo test -p oer-tool-vendor-ieee802154-esp32s31-cli --features multipan
```

## Software coexistence

`--features sw-coex` builds the vendor driver with
`CONFIG_ESP_COEX_SW_COEXIST_ENABLE`, the default of a build that also enables
Wi-Fi or Bluetooth, and gives the engine the driver's default scene levels.
The driver's `esp_coex_ieee802154_txrx_pti_set` and
`esp_coex_ieee802154_ack_pti_set` calls are compared as `coex` records with
their level. The port resolves the levels against a table in which level `n`
has priority `n`, so each published priority names the level the engine
chose; the level-to-priority resolution belongs to libcoexist and is tested
with the HAL. The test that pins the build without coexistence runs only
without the feature.

```console
cargo test -p oer-tool-vendor-ieee802154-esp32s31-cli --features sw-coex
```

## Limits

The stand exercises the driver's software behavior against scripted register
values. It makes no claim about register effects, hardware timing, concurrent
interrupts or RF behavior: register effects of the LL accessors are compared
separately with the PAC, and hardware behavior belongs to HIL.
