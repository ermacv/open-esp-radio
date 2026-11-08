# ESP32-S31 vendor UDP receiver

This ESP-IDF application turns an ESP32-S31 board into a vendor Wi-Fi
station that joins the stand's access point and counts the host's UDP load,
so the [Wi-Fi load scenario](../../../../hil/scenarios/wifi-load/README.md)'s
RX levels run against the vendor driver and IP stack on the same board as
against the open station. The radio starts only on `JOIN`: a boot leaves it
off.

## Build and run

The peer is the ESP-IDF firmware catalog entry `wifi-udp-rx-peer`
(`firmware.toml`), built against the ESP-IDF revision and ESP32-S31 archives
pinned in
[`chips/esp32s31/vendor/artifacts.toml`](../../../../chips/esp32s31/vendor/artifacts.toml),
with the HIL partition table (the application in `ota_0`) and the stand's
16 MB flash:

```console
cargo fw idf build wifi-udp-rx-peer
OER_WIFI_LOAD_VENDOR=<board> OER_WIFI_LOAD_MBPS=100 cargo hil run wifi-load
```

`OER_WIFI_LOAD_VENDOR` makes the scenario's program run its RX levels
against this peer on `<board>` instead of its cases: it leases the board,
the router and the air exclusively, writes the peer's image bundle unless
the board runs it (the LAB service journals it; the next HIL run writes its
own image again), resets the board, applies the router's station profile
with a generated network and hands that network to the peer in one `JOIN`
line of its console input. The network's name and passphrase are never
printed, stored or put in an argument; every console line the host keeps
has them removed.

`sdkconfig.defaults` sets the open station's receive conditions: AMPDU RX
on with a 16-MPDU Block Ack window, 32 static RX buffers (the open
station's large receive slots), power save off, HT only, 40 MHz, country
`DE` (channels 1 to 13). The IP stack's UDP and TCP/IP mailboxes hold 64
like ESP-IDF's iperf example for the ESP32-S31, and its statistics are on.
The vendor's reception statistics (`CONFIG_ESP_WIFI_ENABLE_WIFI_RX_STATS`)
are compiled in and enabled at `JOIN`.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output. At start the peer prints
`@READY protocol=1 target=esp32s31 app=wifi-udp-rx` and `@ADDR <its station
address>`. Each command is answered by `@OK <command>` or `@ERR <command>
<reason>`; a refused vendor call is named with its `esp_err_t`.

| Command | Effect |
| --- | --- |
| `SYNC` | Print `@READY` and `@ADDR` again. |
| `JOIN <channel> <ssid> <passphrase>` | Once per boot: join the WPA2 network on channel 1–13, HT40, power save off, and wait up to 30 s for its DHCP address, rejoining after a disconnection meanwhile. Prints `@JOINED channel=<n> second=<n> rssi=<dBm> phymode=<n> ip=<a.b.c.d> disconnects=<n>` with the vendor's codes. A later disconnection prints `@LOST reason=<code>` and is not rejoined. |
| `RX <run> <port> <delay_ms> <window_ms> <drain_ms>` | Answer at once and count run `run`'s load datagrams on UDP `port` for `delay_ms + window_ms + drain_ms`, those within the window apart, and echo every probe (an echo request of any run) to its source as an echo reply. Clears the vendor's hardware and reception statistics. |
| `REPORT <run>` | Wait for run `run` to end, then print its counts and the vendor's statistics since its `RX` (below). |
| `DUMP` | `esp_wifi_statis_dump(WIFI_STATIS_ALL)` with INFO logging around it, between `@DUMP begin` and `@DUMP end error=<code>`. |

`REPORT` prints:

- `@RX`: the load header's count, the
  [tally](../../../../hil/services/load/src/tally.rs) the image and the host
  keep: distinct `datagrams` and `bytes`, those within the window
  (`window_datagrams`, `window_bytes`), `lost` (sequences up to the highest
  that never arrived), `reordered`, `duplicates`, `untracked` (at or past the
  peer's 2^18 sequences), `highest`, `foreign` datagrams, probes `echoed` and
  `echo_refused`, `receive_errors` of the socket, the first and last
  datagram's time from `RX` (`first_us`, `last_us`) and `window_us`.
- `@LWIP`: the IP stack's `link`, `ip` and `udp` received and dropped
  frames since `RX`, and `udp_memerr`.
- `@WIFIRX`: `esp_wifi_get_rx_statistics` for TID 0 (`legacy`, `ht`,
  `ht_retry`, `rx_isr`, `rx_nblks`, …).
- `@HWRX`: `esp_test_get_hw_rx_statistics`, the MAC's receive counters
  (`rx_fcs_err`, `rx_mpdu`, `rx_datasuc`, `rx_buf_fullcnt`,
  `rx_fifo_ovfcnt`, the PHY's `brx_err`/`nrx_err` causes, …). Most are
  16-bit counts as the vendor holds them: a long window may wrap them.
- `@LINK channel=<n> second=<n> rssi=<dBm> disconnects=<n>`, or `@LINK down`.
