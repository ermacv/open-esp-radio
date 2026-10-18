# ESP32-C5 Bluetooth LE Direct Test Mode reference peer

This ESP-IDF application turns an ESP32-C5 into the reference Bluetooth LE
Direct Test Mode peer of the HIL cell. The vendor BLE Controller owns the
radio; the application only sends it the standard HCI test commands over VHCI
and reports their results through a line protocol on the console, so DTM
exchanges with the device under test run against the vendor implementation.

## Build and flash

The peer is a [firmware catalog](../../host/stand.md#esp-idf-firmware-catalog) entry
(`firmware.toml`). It builds against the ESP-IDF revision and the
`esp32c5-bt-lib` Controller archive pinned in
[`verification/esp32c5/artifacts.toml`](../../../verification/esp32c5/artifacts.toml):

```console
cargo fw flash ble-dtm-peer --device <MAC|PORT>
```

The tracked `sdkconfig.defaults` selects the Controller-only Bluetooth build
with its RAM HCI interface and Direct Test Mode (without it the Controller
answers the test commands with Unknown HCI Command), the USB Serial/JTAG console the stand uses and a
reproducible build.

### USB Serial/JTAG resets

ESP-IDF leaves a clock setting in the ESP32-C5's LP domain that makes a
USB Serial/JTAG reset of rev 1.0 boot into UART/SDIO download with USB dead;
[Hardware errata](../../../docs/hardware-errata.md#esp32-c5-rev-10-a-usb-serialjtag-reset-boots-into-uartsdio-download)
describes it. The peer never enters the PMU MODEM state, so it restores the
ROM's ACTIVE-only analog I2C master clock map at start and after the
Controller enable. The runner takes a running peer over with `SYNC` instead
of a reset.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output. At start the peer resets the Controller and
prints:

```text
@READY protocol=1 target=esp32c5
```

Each command is answered by `@OK <command>` or `@ERR <command> <reason>`,
where the reason is `invalid`, `timeout` or the HCI status as
`status=0x<nn>`.

| Command | Effect |
| --- | --- |
| `RX <version> <channel> <phy>` | LE Receiver Test v1 or v2 on RF channel 0–39. `phy` is `1M`, `2M` or `CODED` (either coding); version 1 accepts only `1M`. |
| `TX <version> <channel> <length> <pattern> <phy>` | LE Transmitter Test v1 or v2 with a payload of `length` octets (0–255, at most 37 for version 1) and HCI payload pattern 0–7. `phy` is `1M`, `2M`, `S8` or `S2`; version 1 accepts only `1M`. |
| `END` | LE Test End: prints `@END packets=<n>` before the reply. After a receiver test `n` is the received packet count; after a transmitter test this Controller reports the packets it sent. |
| `RESET` | HCI Reset, which also ends a test without a count. |
| `SYNC` | HCI Reset, then print `@READY` again: the host takes over a running peer without resetting the chip. |

## Host driver

The host driver, `oer_hil_family_bluetooth::fixture::dtm_peer::DtmPeer`, was
removed with the Bluetooth HIL family in  (tag `legacy-radio-2026-10-16`);
 brings an interop driver back. It drove this protocol from a scenario: it
took the running peer over with `SYNC`, checked the protocol version, refused
arguments outside the table above before sending them, and returned the `@END`
packet count. A scenario that used the peer named `ble-dtm-peer` as its peer
image, so the runner claimed the run's peer board from the stand file's pool in
the run's lease and brought it to the current catalog build first.
