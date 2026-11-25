# ESP32-S31 IEEE 802.15.4 vendor reference

This ESP-IDF application runs the vendor IEEE 802.15.4 driver
(`esp_ieee802154_*`) on the ESP32-S31 behind a line protocol on the console,
for on-air comparison with the open driver and for the IEEE 802.15.4 points
of the calibration cross-check. Its catalog image is
`ieee802154-vendor-reference` (`firmware.toml`); the register model's
evidence cites its build.

## Line protocol

Lines end with `\n`. Every protocol line from the application starts with `@`; the
host ignores any other output, such as boot messages. Hexadecimal bytes are
lowercase on output and accepted in either case on input.

At start the application prints:

```text
@READY protocol=1 target=esp32s31
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
| `SYNC` | Stop a burst, disable and enable the driver, clear both pending tables and pending reports, then print `@READY` again: the host takes over a running application without resetting the chip. |
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
