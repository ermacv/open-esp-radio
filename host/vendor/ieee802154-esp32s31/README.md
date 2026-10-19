# ESP32-S31 IEEE 802.15.4 host stand

This workspace runs the public ESP-IDF IEEE 802.15.4 driver on the host so the
production port can be compared with the real vendor behavior. Unlike the PHY,
Wi-Fi and Bluetooth libraries, this driver is published as source, so the stand
compiles it directly instead of executing a captured binary. The
[driver port map](../../../../docs/vendor/esp32s31/ieee802154-driver-port.md) identifies the pinned
sources and their production owners.

## What is compiled and what is recorded

The chip's [`artifacts.toml`](../../artifacts.toml) pins every translation unit and
every ESP-IDF header the build uses, with its SHA-256 at the pinned revision.
[`build.rs`](build.rs) refuses to build unless each file matches. The driver
sources are compiled unmodified with the Kconfig defaults in
[`shim/include/sdkconfig.h`](shim/include/sdkconfig.h).

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
cargo verification fetch esp32s31
```

downloads every pinned ESP-IDF file of source `esp-idf` into
`target/vendor/esp-idf/<revision>/`, which keeps the checkout's relative
layout, and verifies it. The build reads that directory; `OER_ESP_IDF_DIR`
may name a full checkout at the pinned revision instead. Either way every
file must match its pinned SHA-256. The stand needs a host C compiler.

## Running

```console
cargo test -p oer-vendor-ieee802154-esp32s31
cargo run -p oer-vendor-ieee802154-esp32s31 -- list
cargo run -p oer-vendor-ieee802154-esp32s31 -- run transmit-with-ack
cargo run -p oer-vendor-ieee802154-esp32s31 -- compare transmit-with-ack
cargo run -p oer-vendor-ieee802154-esp32s31 -- shard --index <directory>
```

`shard` compares every catalog scenario, each in its own process, and writes
the stand's evidence shard `ieee802154-host` through `oer-vendor-evidence`
when all of them compare MATCH; `compare` exits 0 for MATCH, 1 for DIFF and 2
for INCOMPLETE. `cargo verification evidence --chip esp32s31 ieee802154-host` runs
it as the stand's producer.

The compiled driver keeps its state in C globals, so each process runs one
scenario. `run` prints the scenario's boundary trace, one record per line;
buffer addresses appear as `tx#n` (the `n`th transmitted frame) and `buf#n`
(the `n`th distinct driver buffer). The [tests](tests/catalog.rs) check the
catalog against expectations read from the pinned source.

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
chip knowledge's `ESP32S31_TX_POWER_LEVELS` (`oer-esp32s31-tables`); the stand links no HAL. The tests require `MATCH` for every catalog
scenario.

The `recent-rssi` scenario reads `esp_ieee802154_get_recent_rssi` disabled,
idle, receiving, awaiting an ACK and asleep, over receive-information images
with nonzero upper bits. The production read is the runtime's over the PAC's
baseband capability and passes no engine or MAC accessor, so the port
records nothing for it: `MATCH` means the vendor call leaves no
register-layer access in any state either. The value is not compared; a test
pins it to the signed low byte of one `bt_bb_get_cur_rx_info` call, the
geometry the register model publishes for the PAC read.

`cargo verification evidence --chip esp32s31 ieee802154-host` runs `compare` for every catalog
scenario and, only when all of them MATCH, writes the stand's evidence shard `ieee802154-host.json` into the
chip's derived evidence index `target/verification/esp32s31/evidence`: one entry per
scenario (source `esp-idf`, production `oer_espressif_ieee802154_engine::engine`), the
digests of the pinned ESP-IDF files and of the stand's path-dependency
closure. The stand measures no vendor coverage, production observation or
vendor state, so its entries leave those out.

## Multi-PAN

`--features multipan` builds the vendor driver with
`CONFIG_IEEE802154_MULTI_PAN_ENABLE` and the Kconfig default of two
interfaces, compiles `esp_ieee802154_multipan.c`, constructs the port with the
same interfaces and adds the multi-PAN scenarios. Every scenario is compared
in both builds; the two expectation tests that pin the default build's trace
run only without the feature.

```console
cargo test -p oer-vendor-ieee802154-esp32s31 --features multipan
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
cargo test -p oer-vendor-ieee802154-esp32s31 --features sw-coex
```

## Limits

The stand exercises the driver's software behavior against scripted register
values. It makes no claim about register effects, hardware timing, concurrent
interrupts or RF behavior: register effects of the LL accessors are compared
separately with the PAC, and hardware behavior belongs to HIL.
