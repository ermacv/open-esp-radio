# HIL board I/O

`oer-stand-board` is everything the host does to a board of the stand. It
encodes nothing (the image pipeline, `oer-image`, builds every bundle),
leases nothing (the arbiter does) and journals nothing (the board journal
does); the flash operation, [`oer-hil-flash`](../../hil/host/flash/README.md), puts them
together.

| Module | Owns |
| --- | --- |
| `ports` | The attached USB serial ports, a board's port by its MAC (its `/dev/serial/by-id` link) and the MAC behind a port |
| `port` | `Port`, the one opener of a serial line: by path with `Settings` (line rate, read timeout, modem lines `Kept` or `Released`, how long a busy open is retried); reads, writes, RTS/DTR, clearing input and the descriptor, no protocol. The console, resets, the flash writer, the DUT link's captures and the peer consoles open through it |
| `flash` | The one flash writer: an image bundle's segments through one espflash library connection for the chip its profile names, retries of a failed serial link, unchanged segments skipped after an MD5 comparison, the OTA selection last |
| `Board` | A board of the stand file: `write(bundle, via)` (USB or JTAG), `start` as the chip profile's `[flash] start` says, its reset ladder's rungs, `reset(path)`, its console and its hub power; a board off USB is put into the ROM's download mode before a write |
| `reset` | The USB Serial/JTAG openers and resets and `climb`, the one ladder every recovery and `cargo stand board reset` take; the journal's `RecoveryStep` and `ResetPath` |
| `openocd` | The one OpenOCD: located in the ESP-IDF tools of the shared cache; reset, register reads without a reset, flash programming |
| `power` | Hub port power through `uhubctl` and the one parser of its report |
| `console` | The one serial line reader, bounded captures and the ROM's reset line |

A board's start policy belongs to its chip profile: `reset` (the writer's
hard reset starts the image, esp32s31) or `power-on` (the writer leaves the
ROM in its download mode; a power-on reset of the board's hub port starts
the image, an RTS reset on a board that does not reset by power; an RTS reset
out of download mode starts an esp32c5's image with its USB console silent).

```console
cargo test -p oer-stand-board
```
