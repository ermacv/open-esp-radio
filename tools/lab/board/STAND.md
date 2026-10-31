# HIL board I/O

`oer-lab-board` is everything the host does to a board of the stand. It
encodes nothing (the image pipeline, `oer-image-build`, builds every bundle),
leases nothing and journals nothing; the LAB service (`oer-tool-lab-service`),
its one caller, puts them together for its clients.

| Module | Owns |
| --- | --- |
| `ports` | The attached USB serial ports, a board's port by its MAC (its `/dev/serial/by-id` link) and the MAC behind a port |
| `port` | `Port`, the one opener of a serial line: by path with `Settings` (line rate, read timeout, modem lines `Kept` or `Released`, how long a busy open is retried); reads, writes, RTS/DTR, clearing input and the descriptor, no protocol. The console, resets, the flash writer, the DUT link's captures and the peer consoles open through it |
| `flash` | The one flash writer: an image bundle's segments through one espflash library connection for the chip its profile names, retries of a failed serial link, unchanged segments skipped after an MD5 comparison, the OTA selection last |
| `Board` | A board of the stand file: `write(bundle, via)` (USB or JTAG), `start` as the chip profile's `[flash] start` says, its reset ladder's rungs, `reset(path)`, its console and its hub power; a board off USB is put into the ROM's download mode before a write |
| `reset` | The USB Serial/JTAG openers and resets: the rungs of a board's reset ladder, which the LAB service takes to park a board and `cargo stand board reset` names |
| `openocd` | The one OpenOCD: located in the ESP-IDF tools of the shared cache; reset, register reads without a reset, flash programming |
| `power` | Hub port power through `uhubctl`: on, off, cycles and the proof of power loss; its report's parser is `oer_lab_model::hubs` |
| `console` | The one serial line reader, bounded reads and the ROM's reset line |

A board's start policy belongs to its chip profile: `reset` (the writer's
hard reset starts the image, esp32s31) or `power-on` (the writer leaves the
ROM in its download mode; a power-on reset of the board's hub port starts
the image, an RTS reset on a board that does not reset by power; an RTS reset
out of download mode starts an esp32c5's image with its USB console silent).

```console
cargo test -p oer-lab-board
```
