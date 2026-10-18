# `cargo fw`

Builds the staged images of `firmware/apps`, writes them into a stand board's
flash and runs them to a verdict.

```console
cargo fw build <image>
cargo fw build w1 [--expect-bssid <MAC> --channel <1..13>]
cargo fw flash <image> --board <id|MAC>
cargo fw run <image> --board <id|MAC> [--timeout 30s]
cargo fw run w1 --board <id|MAC> --expect-bssid <MAC> --channel <1..13> [--timeout 90s]
cargo fw monitor --board <id|MAC>
cargo fw boards
cargo fw power-cycle --board <id|MAC>
```

## Build

`build` compiles the image's package as the stage-two runtime
(`riscv32imafc-unknown-none-elf`, release), flattens its load image from
`__runtime_image_start` to `__runtime_payload_end` and stores the payload CRC
in its stage-two header (`oer-espressif-staged-layout`'s `stage_two`). The
packed runtime goes to the bootstrap through `PSRAM_RUNTIME_BIN`; an unchanged
runtime keeps its file, so the bootstrap is not rebuilt. The `espflash`
library then encodes the bootstrap ELF for the `[flash]` map of
`firmware/platform/esp32s31/chip.toml`: the DIO bootloader from espflash's
resources, the partition table, the QIO application in `ota_0` and the OTA
selection of `ota_0`. Everything lands in `target/fw/<image>/`, listed by
`segments.toml` in write order.

Image `w1` takes the stand AP's BSSID and its channel: `--expect-bssid`
(six hex octets separated by `:` or `-`, any case; a multicast or zero
address is refused) and `--channel` (1 to 13), or, when a flag is absent, the
variables `OER_W1_EXPECT_BSSID` and `OER_W1_CHANNEL`; a flag and its variable
that disagree are refused. `run w1` needs both, `build w1` both or neither
(neither builds the unconfigured image, which fails at once). No other image
takes them. They reach only the runtime build of `w1`, as `OER_W1_CHANNEL`
and `OER_W1_EXPECT_BSSID`; every other cargo build runs with both removed.
The BSSID is never shown whole: messages, the lock file's command line and
`Debug` show `xx:xx:xx:xx:F6:D0`, and clap's errors do not repeat it.

The build warns, and never fails, when the runtime places an interrupt table
handler, the panic entry or hook, or a `.dma.*` or `.critical.*` section
outside internal SRAM (`placement.txt`).

## Boards

Boards and hubs come from the owner's stand file,
`~/.config/open-esp-radio/stand.toml` (or `OER_STAND_FILE`); only `schema`,
`[[hub]]` and `[[board]]` are read. Attached boards are found in sysfs as
Espressif USB Serial/JTAG devices (`303a:1001`), whose serial number is the
chip's MAC. Only ESP32-S31 boards are reset or written; other chips are only
listed.

A board is used by one process at a time: an exclusive lock on
`$XDG_RUNTIME_DIR/oer-fw/<MAC>.lock`, shared by every checkout, which names
its holder when it refuses.

- `flash` writes the segments through one espflash connection, skipping each
  one whose flash contents already match (MD5), the OTA selection last, and
  hard-resets the board.
- `run` builds and writes the image, leaves the chip in the flasher stub,
  opens the console and resets the chip into the image with an RTS pulse, then
  reads until `OER-RESULT: pass …` (exit 0), `OER-RESULT: fail …` (1) or the
  timeout (2). The console goes to `target/fw/runs/<time>-<image>/log.txt`.
- `run w1` also reads the report block the image prints after its exit,
  `OER-W1-BEGIN v=1` to `OER-W1-END lines=<n>` (one `keyword key=value …`
  line each, every value validated), and recomputes each gate from the
  block's other lines: `beacons` (the stand's beacons less duplicates
  against ⌊8·window_us / (10·bi_tu·1024)⌋; `stand-absent`,
  `stand-not-tracked`, `zero-interval` or `beacons`), `channel` (the applied
  channel is the configured one and `--channel`), `exit` (`StopRequested`,
  `Halted`), `walker_restart` (`pool-not-exhausted` is unexercised,
  `no-frames-after-release` a fail), `irq_stack` (4096 bytes free),
  `service_errors` and `unserviced` (zero). It compares them with the
  image's `w1.gate` lines, `w1.verdict` and the `OER-RESULT:` line, whose
  fail reasons each start with a gate's name (`walker-restart not
  exercised`), and the block's `channel=` and masked `stand=` with the flags,
  which catches a stale build. Exit 0 when both pass, 1 when both fail, 2 on
  the timeout and 3 when the block is missing or malformed or the image and
  the host disagree on anything. A summary follows: the gates as both see
  them and the beacon gaps that overlap a PHY tracking tick (the stand's
  smallest handoff lag maps the frame stamps onto the ticks' clock).
- `monitor` opens the console without a reset: RTS is released before DTR.
- `power-cycle` switches the board's hub port off and on with `uhubctl`, only
  when the stand file lists the port as switchable and not protected and the
  board's USB location in sysfs is that very port; it then waits for the
  board to return.
