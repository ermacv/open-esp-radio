# ESP32-C5 IEEE 802.15.4 reference peer

This ESP-IDF application turns an ESP32-C5 into the reference IEEE 802.15.4
peer of the HIL cell. The vendor driver (`esp_ieee802154_*`) owns the radio;
the application only drives it through a line protocol on the console, so
exchanges with the device under test run against the vendor implementation.

## Build and flash

The peer is a [firmware catalog](../../../tools/hil/run/stand.md#esp-idf-firmware-catalog) entry
(`firmware.toml`). It builds against the ESP-IDF revision and ESP32-C5 archives
pinned in
[`chips/esp32c5/vendor/artifacts.toml`](../../../chips/esp32c5/vendor/artifacts.toml),
installed below `target/` apart from any user installation:

```console
cargo fw flash ieee802154-peer --device <MAC|PORT>
```

This builds the image, flashes it under a lease of that board only and records
it in the board journal as `ieee802154-peer`. The tracked
`sdkconfig.defaults` selects the USB Serial/JTAG console the stand uses and a
reproducible build.

The peer is a board of the stand file's pool with the `peer` role
(`--peer-board ID` names it when there are several). The board is shared with
other consumers. Before a
run's first IEEE 802.15.4 peer scenario, `cargo hil run` brings the board up
to the current catalog build of the scenario's peer image within the run's
lease, which holds and locks the peer board: the flash operation's catalog
flash builds the
image (from the shared ESP-IDF cache, quickly once built) and flashes and
journals it unless the board's newest recorded flash is that image with the
same digest, so a fixed peer reaches the board without a manual flash. `doctor` and `fixture check` do not flash: they report
the other image and who flashed it. A board without a recorded flash is
accepted; the peer's `@READY` handshake then decides.

### USB Serial/JTAG resets

ESP-IDF leaves a clock setting in the ESP32-C5's LP domain that makes a
USB Serial/JTAG reset of rev 1.0 boot into UART/SDIO download with USB dead;
[Hardware errata](../../../docs/hardware-errata.md#esp32-c5-rev-10-a-usb-serialjtag-reset-boots-into-uartsdio-download)
describes it. The peer never enters the PMU MODEM state, so on the ESP32-C5 it
restores the ROM's ACTIVE-only analog I2C master clock map at start and after
each radio enable, and USB resets and flashes over it work. The runner still
takes a running peer over with `SYNC` instead of a reset.

### A wedged USB console

The peer's USB Serial/JTAG console can wedge with the board still
enumerated: every write and every control request (RTS, DTR) fails with
`EPIPE` ("Broken pipe"), the hub lists the device without its string
descriptors (`[303a:1001]` with no product or serial), and neither an RTS
nor a JTAG reset nor `espflash` reaches it. Before every peer scenario
the run's preflight sends `SYNC` under the run's lease, so such a console
fails the scenario's precondition, pointing here, instead of breaking it
midway. Firmware is not the cause, so reflashing does not help; power-cycling only the peer's own hub
port does: `cargo stand board reset esp32c5 --via power`, for a board whose reset ladder has
`power` on a switchable hub port of the stand file. A lease runs no `uhubctl` itself.
`cargo hil peer send esp32c5 SYNC` answers `@OK SYNC` once it is back.

A run of `system-watchdog --chip esp32c5` on this board always leaves it
wedged: the scenario's intentional watchdog reset drops the C5's USB
30–75 s into the run, and the board comes back half-enumerated as above.
Until its port is power-cycled, the next peer scenario's restore fails at
`espflash` ("Failed to connect to the device") or its preflight `SYNC`
times out. Whoever runs that scenario on the peer board cycles the port
under the same lease or right after it, before releasing the board to peer
scenarios.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output, such as boot messages. Hexadecimal bytes are
lowercase on output and accepted in either case on input.

At start the peer prints:

```text
@READY protocol=1 target=esp32c5
```

Each command is answered by `@OK <command>` or `@ERR <command> <reason>`.

| Command | Effect |
| --- | --- |
| `CFG <channel> <panid> <short> <ext> <promiscuous> <power>` | Channel 11–26; PAN ID and short address as four hex digits, big-endian; extended address as sixteen hex digits in over-the-air (little-endian) order; promiscuous `0`/`1`; transmit power in dBm. Receive-when-idle is enabled. |
| `RX` | Enter receive mode. |
| `SLEEP` | Leave receive mode. |
| `TX <cca> <mac>` | Transmit the MAC bytes (without FCS) given in hex, after a CCA when `cca` is `1`. The frame's own acknowledgement-request bit selects whether an ACK is awaited. |
| `TXAT <delay_us> <mac>` | Transmit without CCA `delay_us` (at most 1000000) microseconds from now through `esp_ieee802154_transmit_at` on the `esp_timer` clock. |
| `BURST <mac>` | Transmit the MAC bytes back to back without CCA until `BURST STOP`; the ends of these transmissions are counted, not reported. |
| `BURST STOP` | Stop the burst, wait for its last transmission and print `@BURST sent=<n> failed=<n>` before the reply. |
| `PENDING <mode>` | Set the automatic frame-pending mode: `0` disabled, `1` enabled, `2` enhanced, `3` Zigbee. |
| `PENDING ADD <short>` | Add a short address (big-endian hex) to the pending table. |
| `PENDING CLEAR` | Clear the short-address pending table. |
| `ED <symbols>` | Run one energy detection of the given number of 16 µs symbols. |
| `SYNC` | Stop a burst, disable and enable the driver, clear both pending tables and pending reports, then print `@READY` again: the host takes over a running peer without resetting the chip. |
| `OFF` | Disable the driver (`esp_ieee802154_disable`): the MAC, BTBB and the PHY client are released, and RF closes after the last client. |
| `ON` | Enable the driver again (`esp_ieee802154_enable`); send `CFG` before the next operation. |
| `PEEK <address>` | Read the 32-bit word at the word-aligned device address (eight hexadecimal digits) and print `@PEEK <address> <value>`, for register cross-checks. A register whose clock domain is off may reset the chip. |
| `ANALOG <block> <register>` | Read one analog-I2C register (two hexadecimal digits each) through ESP-IDF's analog-I2C driver and print `@ANALOG <block> <register> <value>`. |

Events, printed when the driver reports them:

| Event | Meaning |
| --- | --- |
| `@RX <mac> rssi=<dBm> lqi=<n> pending=<0/1> ch=<n>` | A frame was received; `<mac>` excludes PHR and FCS; `pending` reports whether the automatic ACK carried frame pending. |
| `@TXDONE ack=-` | A frame without acknowledgement request was sent. |
| `@TXDONE ack=<mac> pending=<0/1> rssi=<dBm> lqi=<n>` | A frame was acknowledged; `<mac>` is the ACK without FCS. |
| `@TXFAIL <code>` | The driver reported `esp_ieee802154_tx_error_t` `<code>`. |
| `@ED <dBm>` | The energy detection result. |

Driver callbacks run in interrupt context; the application copies each
report into a bounded queue and prints it from a task.
