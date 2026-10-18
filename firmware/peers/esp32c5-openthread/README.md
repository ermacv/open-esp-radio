# ESP32-C5 Thread reference peer

This ESP-IDF application runs ESP-IDF's OpenThread stack as a full Thread
device on the ESP32-C5's native IEEE 802.15.4 radio, and lets the HIL host
drive it over a line protocol on the USB Serial/JTAG console. It is the
reference Thread node for the device under test's OpenThread radio: it forms
a network as its leader, hands out the active operational dataset, reports
its role and addresses, and exchanges UDP datagrams.

It shares the ESP32-C5 board with the
[IEEE 802.15.4 peer](../esp32c5-i/README.md); each scenario names
the image it needs, and the runner restores that image before it runs.

## Build and flash

The application builds against the ESP-IDF revision and ESP32-C5 archives pinned in
[`verification/esp32c5/artifacts.toml`](../../../verification/esp32c5/artifacts.toml),
without the OpenThread CLI, the ESP-IDF console or the lwIP glue:

```console
cargo fw flash openthread-peer --device <MAC|PORT>
```

Like the IEEE 802.15.4 peer, it keeps the ROM's I2C master clock map so a
USB Serial/JTAG reset of the running peer boots normally; see
[USB Serial/JTAG resets](../esp32c5-i/README.md#usb-serialjtag-resets).

## Line protocol

After boot the peer prints `@READY protocol=1 target=esp32c5 stack=openthread`.
Each command is answered by `@OK <command>` or `@ERR <command> <reason>`,
where `<reason>` is OpenThread's error name.

| Command | Meaning |
| --- | --- |
| `FORM <channel> <panid>` | Erase the stored network, create a new one on the channel (11–26) and PAN ID (decimal or `0x` hex) with fresh keys, and start Thread; the peer becomes its leader. |
| `DATASET` | Print `@DATASET <hex>`: the active operational dataset TLVs. |
| `STATE` | Print `@STATE role=<role> rloc16=<hex> eid=<address>`: OpenThread's device role name, the RLOC16 and the mesh-local EID. |
| `UDP OPEN <port>` | Open the UDP socket, bound to the port on the Thread interface. |
| `UDP SEND <address> <port> <hex>` | Send the bytes (at most 128) from the socket to the IPv6 address and port. |
| `SYNC` | Close the socket and disable Thread and IPv6, then print `@READY` again: the host takes over a running peer without resetting the chip. |
| `OFF` | Leave the network and stop OpenThread, which disables the IEEE 802.15.4 driver and closes RF; the peer then answers only after a reset. |

Events:

| Event | Meaning |
| --- | --- |
| `@UDPRX <address> <port> <hex>` | The socket received a datagram from the address and port. |

OpenThread runs in its own task; every command takes the OpenThread lock,
and received datagrams are printed from that task.
