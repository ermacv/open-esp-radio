# ESP32-S31 vendor UDP receiver

This ESP-IDF application turns an ESP32-S31 board into a vendor Wi-Fi
station that joins the stand's access point and counts the host's UDP load,
so the [Wi-Fi load scenario](../../../../hil/scenarios/wifi-load/README.md)'s
RX levels run against the vendor driver and IP stack on the same board as
against the open station. The radio starts only on `JOIN`: a boot leaves it
off.

## Variants

Two catalog entries build this application against the ESP-IDF revision and
ESP32-S31 archives pinned in
[`chips/esp32s31/vendor/artifacts.toml`](../../../../chips/esp32s31/vendor/artifacts.toml),
with the HIL partition table (the application in `ota_0`) and the stand's
16 MB flash. They share `sdkconfig.defaults` and the source
`main/udp_rx.c` and differ only in how the load is consumed:

| Image | Project | Consumer |
| --- | --- | --- |
| `wifi-udp-rx-peer` | this directory | `socket`: a receiver task reads a BSD socket, copying each datagram out of the IP stack |
| `wifi-udp-rx-fast-peer` | [`../esp32s31-wifi-udp-rx-fast`](../esp32s31-wifi-udp-rx-fast) | `fast`: lwIP's thread counts each datagram through the raw UDP API and frees its buffer at once, without the socket queue, the task switch and the copy (`OER_UDP_RX_FAST`) |

The fast project builds from this directory's `sdkconfig.defaults` and
sources and names each of them in its `firmware.toml`'s `shared`, so they
take part in its build recipe: a change to one makes both entries not
current until they are built again.

## Build and run

```console
cargo fw idf build wifi-udp-rx-peer
cargo fw idf build wifi-udp-rx-fast-peer
OER_WIFI_LOAD_VENDOR=fast OER_WIFI_LOAD_MBPS=100 cargo hil run wifi-load --board <board>
```

`OER_WIFI_LOAD_VENDOR=socket|fast` makes the scenario's program run its RX
levels against this peer instead of its cases (`--list`, `--case path|rx`
and `--board` as for the cases, read before the stand). It leases the
board, the router and the air exclusively, writes the variant's image
bundle unless the board runs it (the LAB service journals it; the next HIL
run writes its own image again), resets the board, applies the router's
station profile with a generated network and hands that network to the peer
in one `JOIN` line of its console input. The network's name and passphrase
are never printed, stored or put in an argument; every console line the host
keeps has them removed.

`sdkconfig.defaults` sets the open station's receive conditions: AMPDU RX
on with a 16-MPDU Block Ack window, 32 static RX buffers (the open
station's large receive slots), power save off, HT only, 40 MHz, country
`DE` (channels 1 to 13). The IP stack's UDP and TCP/IP mailboxes hold 64
like ESP-IDF's iperf example for the ESP32-S31, and its statistics are on.
The vendor's reception statistics (`CONFIG_ESP_WIFI_ENABLE_WIFI_RX_STATS`)
are compiled in and enabled at `JOIN`.

## The link and the counters

One context owns the link: the console task. The Wi-Fi and IP event
handlers only post their events (associated, disconnected, address taken,
address lost) to its queue, in the event loop's order, and a handler that
cannot post one marks the state unknown for good. The state machine
([`main/link.c`](main/link.c), no SDK call) goes up only when every event
applied up to the last one pending after the joined link's facts were read
still says associated and addressed, and none of those events changed the
link since the read began (its epoch): a disconnection queued behind the
address is no join, and facts read on a link that went and came back are
read again on the new one, within the join's 30 s. Its host test
([`test/link_test.c`](test/link_test.c)) runs in
`cargo test -p oer-hil-scenarios-wifi-load` (`tests/peer_link.rs`) with the
host's C compiler (`$OER_HOST_CC`, else `cc`); a missing compiler fails it.

The IP stack's counters are read in lwIP's own thread (`tcpip_api_call`):
the run task samples them every 100 ms and adds up their 16-bit differences
in 64 bits.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output. At start the peer prints
`@READY protocol=3 target=esp32s31 app=wifi-udp-rx variant=socket|fast` and
`@ADDR <its station address>`. Each command is answered by `@OK <command>` or `@ERR <command>
<reason>`; a refused vendor call is named with its `esp_err_t`.

| Command | Effect |
| --- | --- |
| `SYNC` | Print `@READY` and `@ADDR` again. |
| `JOIN <channel> <ssid> <passphrase>` | Once per boot: join the WPA2 network on channel 1–13, HT40, power save off, and wait up to 30 s for the link to be up, connecting again after a disconnection meanwhile. Prints `@JOINED channel=<n> second=<n> rssi=<dBm> phymode=<n> ip=<a.b.c.d> disconnects=<n>` with the vendor's codes. A failed vendor call, or a failed read of the joined access point, mode or address, refuses the join. A later end of the link prints `@LOST state=<state> reason=<code> disconnects=<n>` and is not rejoined. |
| `RX <run> <port> <delay_ms> <window_ms> <drain_ms>` | Once the last run's end is confirmed, start counting run `run`'s load datagrams on UDP `port` for `delay_ms + window_ms + drain_ms`, those within the window apart, echo every probe (an echo request of any run) to its source as an echo reply, and answer. A run task takes the vendor's counters at the run's start, the window's start and end and the run's end (the MAC's hardware counters are cleared at the window's start and read at its end), then confirms that the counter stopped and completes the run with its id. Refused while a run is active (`running`), and for good once a counter did not confirm its stop (`counter-unconfirmed`). |
| `REPORT <run>` | Wait for run `run`'s completion, which must name it and confirm its stop, then print its counts and counters (below); `unknown-run` for another run. |
| `DUMP` | `esp_wifi_statis_dump(WIFI_STATIS_ALL)` with INFO logging around it, between `@DUMP begin` and `@DUMP end error=<code>`: counts since boot, read after the drain. |

`REPORT` prints, every line with `run=<run>`:

- `@RX`: the load header's count, the
  [tally](../../../../hil/services/load/src/tally.rs) the image and the host
  keep: distinct `datagrams` and `bytes`, those within the window
  (`window_datagrams`, `window_bytes`), `lost` (sequences up to the highest
  that never arrived), `reordered`, `duplicates`, `untracked` (at or past the
  peer's 2^18 sequences), `highest`, `foreign` datagrams, probes `echoed` and
  `echo_refused`, `receive_errors` of the socket, the first and last
  datagram's time from `RX` (`first_us`, `last_us`) and `window_us`.
- `@SPAN`: when the run task took the window's start and end and the run's
  end, from `RX` (`window_from_us`, `window_to_us`, `run_to_us`).
- `@LWIP span=window|run`: the IP stack's `link`, `ip` and `udp` received
  and dropped frames and `udp_memerr` over the window and over the run.
- `@WIFIRX span=window|run`: `esp_wifi_get_rx_statistics` for TID 0
  (`legacy`, `ht`, `ht_noeb` — MPDUs the driver had no buffer for —,
  `ht_retry`, `rx_isr`, `rx_nblks`, …) over the window and over the run.
- `@HWRX span=window`: `esp_test_get_hw_rx_statistics`, the MAC's receive
  counter registers (`rx_fcs_err`, `rx_mpdu`, `rx_datasuc`,
  `rx_buf_fullcnt`, `rx_fifo_ovfcnt`, the PHY's `brx_err`/`nrx_err` causes,
  …) over the window. They are 16-bit or 10-bit fields of the registers as
  the vendor reads them: a count at its field's limit may have wrapped.
- `@LINK state=up channel=<n> second=<n> rssi=<dBm> disconnects=<n>`, or
  `@LINK state=<idle|joining|down|faulted> disconnects=<n>`.

A getter the vendor refused prints its line with `error=<code>` instead.
