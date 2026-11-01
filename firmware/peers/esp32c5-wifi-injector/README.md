# ESP32-C5 Wi-Fi frame source

This ESP-IDF application turns an ESP32-C5 into a vendor Wi-Fi transmitter
of unicast data frames at one fixed PHY rate. The vendor driver owns the
radio in station mode and never associates; the application drives its raw
transmission (`esp_wifi_80211_tx`) with the rate fixed by
`esp_wifi_config_80211_tx`, and counts how the vendor MAC ended each frame:
acknowledged or not. A receiver under test thus hears frames of a chosen
format, and the peer tells whether that receiver acknowledged them.

## Build and flash

The peer is an ESP-IDF firmware catalog entry (`cargo fw idf list`)
(`firmware.toml`), built against the ESP-IDF revision and ESP32-C5 archives
pinned in
[`chips/esp32c5/vendor/artifacts.toml`](../../../chips/esp32c5/vendor/artifacts.toml).
The ESP32-S31 vendor reference
[`wifi-injector-reference`](../../vendor/esp32s31/wifi-injector-reference)
builds the same application for an S31 board of the stand:

```console
cargo fw idf build wifi-injector-peer
cargo fw idf build wifi-injector-vendor-reference
```

`cargo fw flash` writes only the S31 images of `firmware/apps` and
`hil-scenarios/images`, so a leased command writes a catalog build with
ESP-IDF's `esptool` from the cache's tools (`write-flash @flash_args` in the
build directory, `--chip`, the board's `/dev/serial/by-id` port), and the
lease journals it:

```console
cargo stand lease --board <board> --flashed <image> \
  --application <build directory>/<application>.bin --device <board> -- <write command>
```

On an S31 the LAB service resets the board into the image when the lease
ends. An ESP32-C5 rev 1.0 that an RTS reset leaves in its UART download mode
([hardware errata](../../../docs/hardware-errata.md#esp32-c5-rev-10-a-usb-serialjtag-reset-boots-into-uartsdio-download))
needs a power-on reset of its hub port.

`sdkconfig.defaults` selects the USB Serial/JTAG console the stand uses, a
reproducible build and warnings in the log, so a refused vendor call names
its reason on the console (the host ignores lines without `@`). Power save
stays off, so the PMU never enters its MODEM state; on the ESP32-C5 the peer
keeps the ROM's analog I2C master clock map like the
[IEEE 802.15.4 peer](../esp32c5-ieee802154/README.md#usb-serialjtag-resets).

Commands reach the peer through the LAB service:

```console
cargo stand lease --board <peer> --air exclusive -- \
  cargo stand board send <peer> --timeout 30s SYNC \
  'MODE 13 40- mcs7l' 'TX 2000 5000 200 <addr1> <addr2> <addr3> 0'
```

A peer that transmits uses the air: its commands run under a lease that
holds it.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output. At start the peer prints
`@READY protocol=1 target=esp32c5` and `@ADDR <its station address>`. Each
command is answered by `@OK <command>` or `@ERR <command> <reason>`; a
refused vendor call is named with its `esp_err_t`, e.g. `start=0x3002`.

| Command | Effect |
| --- | --- |
| `SYNC` | Print `@READY` and `@ADDR` again. |
| `MODE <channel> <width> <rate>` | Start the station interface with power save off (once), tune it, then fix the raw-transmission rate: channel 1–13 (country `DE`); width `20`, `40-` (secondary channel below) or `40+`; rate `1m`, `2m`, `5.5m`, `11m` (DSSS/CCK, long preamble), `6m` … `54m` (OFDM) or `mcs0l` … `mcs7l`, `mcs0s` … `mcs7s` (HT, one stream, long or short guard interval; HT20 at width `20`, HT40 otherwise). Prints `@MODE channel=<n> second=<n> phymode=<n> rate=<code>` with the vendor's codes. The pinned vendor refuses the rate (`config_80211_tx=0xffffffff`) before the interface is started and an HT40 rate before it is 40 MHz wide. |
| `TX <ms> <interval_us> <length> <addr1> <addr2> <addr3> <ds>` | For `ms` milliseconds (at most 600000), send a non-QoS data frame every `interval_us` microseconds (at most 1000000), at most four unfinished at once: `length` bytes from the MAC header to the end of the body (32 to 1500, FCS excluded), addresses as `aa:bb:cc:dd:ee:ff`, `ds` the ToDS (1) and FromDS (2) flags. The body is LLC/SNAP with the local experimental EtherType `0x88b5` and a counting pattern; the driver numbers the sequence. Then prints `@TX sent=<n> refused=<n> acked=<n> failed=<n> pending=<n> other=<n> last_refusal=<code>`. |

`sent` counts the frames the driver took, `refused` those it refused
(`last_refusal` its last code), `acked` and `failed` the completions it
reported for frames to `addr1` as acknowledged or not (after its own
retries), `pending` the frames without a completion 500 ms after the last
one, `other` completions for another receiver. The vendor raw path sends
only non-QoS data frames (it refuses QoS data: `unsupport QoS frame type`),
so these test Normal Ack MPDUs outside an A-MPDU.
`MODE` takes effect for the next `TX`; the PHY rate the frames go out at on
the air is the vendor's, which an air observer confirms.
