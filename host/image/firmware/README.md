# `cargo fw`

Builds the staged images of `firmware/apps`, writes them into a stand board's
flash and runs them to a verdict.

```console
cargo fw build <image>
cargo fw flash <image> --board <id|MAC>
cargo fw run <image> --board <id|MAC> [--timeout 30s]
cargo fw monitor --board <id|MAC>
cargo fw router
cargo fw boards
cargo fw power-cycle --board <id|MAC>
```

## Build

`build` compiles the image's package as the stage-two runtime
(`riscv32imafc-unknown-none-elf`, release), flattens its load image from
`__runtime_image_start` to `__runtime_payload_end` and stores the payload CRC
in its stage-two header (`oer-platform-staged-layout`'s `stage_two`). The
packed runtime goes to the bootstrap through `PSRAM_RUNTIME_BIN`; an unchanged
runtime keeps its file, so the bootstrap is not rebuilt. The `espflash`
library then encodes the bootstrap ELF for the `[flash]` map of
`firmware/platform/esp32s31/chip.toml`: the DIO bootloader from espflash's
resources, the partition table, the QIO application in `ota_0` and the OTA
selection of `ota_0`. Everything lands in `target/fw/<image>/`, listed by
`segments.toml` in write order.

`build w1` builds the unconfigured image (it prints
`OER-RESULT: fail w1 unconfigured` when run; CI builds it so). Only
`run w1` builds it for the stand, from the stand router (below): the BSSID
and channel of the router's AP and the stand's SSID reach only the runtime
build of `w1`, as `OER_W1_CHANNEL`, `OER_W1_BSSID` and `OER_W1_SSID_HEX`;
every other cargo build runs with all three removed. No image takes a
flag for them.

The build warns, and never fails, when the runtime places an interrupt table
handler, the panic entry or hook, or a `.dma.*` or `.critical.*` section
outside internal SRAM (`placement.txt`).

Every image builds with the compiler's frame records (`-Z emit-stack-sizes`),
moves above the stack policy's `max_move_bytes` warned (`-W
large-assignments`), and through the image linker
(`oer-image-linker` of `host/image/checks`), which refuses bytes in an input section
the boot zeroes. The stack checks then bound the runtime's task and
interrupt stacks and the bootstrap's stack under
`firmware/platform/esp32s31/stack.toml` (`runtime-stack.txt`,
`interrupt-stack.txt`, `bootstrap-stack.txt`). They report and do not fail
the build yet: a failed check prints as a warning.

## Boards

Boards and hubs come from the owner's stand file,
`~/.config/open-esp-radio/stand.toml` (or `OER_STAND_FILE`, mode 0600),
which `oer-lab-router` reads ([host/lab/router/README.md](../../lab/router/README.md): its
schema, what each command reads of it, which values are secret). Attached
boards are found in sysfs as
Espressif USB Serial/JTAG devices (`303a:1001`), whose serial number is the
chip's MAC. Only ESP32-S31 boards are reset or written; other chips are only
listed.

A board is used by one process at a time: `cargo fw` holds it through the
device lock (`oer-lab-device-lock`,
`$XDG_RUNTIME_DIR/open-esp-radio/devices/<MAC>.lock`) that HIL leases and the
stand's flashes take too, shared by every checkout, which names its holder
when it refuses.

- `flash` writes the segments through one espflash connection, skipping each
  one whose flash contents already match (MD5), the OTA selection last, and
  hard-resets the board.
- `run` builds and writes the image, leaves the chip in the flasher stub,
  opens the console and resets the chip into the image with an RTS pulse, then
  reads until `OER-RESULT: pass …` (exit 0), `OER-RESULT: fail …` (1) or the
  timeout (2). The console goes to `target/fw/runs/<time>-<image>/log.txt`.
- `run w1` first brings the stand router to the W1 profile and verifies it
  (below), then builds the image for its AP. It also reads the report block the image prints after its exit,
  `OER-W1-BEGIN v=1` to `OER-W1-END lines=<n>` (one `keyword key=value …`
  line each, every value validated), and recomputes each gate from the
  block's other lines: `beacons` (the stand is the `w1.bss who=stand` line,
  the router AP's BSSID; it must carry the stand's SSID, `ssid_ok=1`; its
  beacons less duplicates against ⌊8·window_us / (10·bi_tu·1024)⌋;
  `stand-absent`, `stand-not-tracked`, `stand-mismatch`, `zero-interval` or
  `beacons`; other BSSIDs carrying the SSID never stand in for an absent
  stand), `channel` (the applied channel is the configured one and the
  router AP's), `exit` (`StopRequested`,
  `Halted`), `walker_restart` (from the first of the two hold cycles,
  `w1.hold cycle=0`: a hold that was not full, `full=0` (every staging
  slot, `reason=all`, or one short and stuck at LAST,
  `reason=stuck-at-last`; both checked against `reached=`, and a stuck
  hold's `end_*=` must show `slots − 1` buffers staged and detached, at
  most one released, every descriptor observed and NEXT zero, or the
  image and the host disagree), is
  unexercised, `pool-not-exhausted`; units received while every lease was held fail
  it, `received-while-held`, as does `no-frames-after-release`; how the
  ring recovered, `w1.reload` and the walker snapshots `w1.exhaust`, one
  per cycle, is report-only), `ownership` (from both `w1.own` lines: any
  `hold_mismatch=`, `dwell_mismatch=`, `release_mismatch=` or `chained=`
  above zero fails it, `foreign-write`; otherwise a `w1.hold full=0` is
  unexercised, `pool-not-exhausted`, and `first_drop=-` too,
  `ring-order`; the host recomputes `order_breaks=` from
  `first_after_release=` and the `arena=` ring size, validates
  `discards=<reason>:<n>,…` and in the summary sets the descriptors the
  list skips against the runner's drops in it), `irq_stack` (4096 bytes
  free), `service_errors` and `unserviced` (zero), `timer_wake` (no
  timer wait of the image returned before its deadline, `woke_early=` of
  every `w1.timer` and `w1.wake` line; `woke-early`), `base_writes` (the
  epoch wrote `RX_DESCRIPTOR_BASE` once, `w1.reload base_writes=1`;
  `base-writes`), `reserved` (`w1.reload reserved_landings=` at most 1
  and `reserved_late_landings=0`; `reserved-landing`), `owner_bit` (both
  `w1.own` lines' `owner_clear=` equal their `held=`; `owner-bit-set`)
  and `walker_on_detached` (`w1.reload walker_on_detached=0`;
  `walker-on-detached`). The `w1.reload`
  keys are exactly `max_attempts limit observations reload_rerings
  rering_failures base_writes reserved_landings reserved_late_landings
  walker_on_detached`,
  and `w1.hold fill_ms=` must lie in 3000..=10000. It compares them with the
  image's `w1.gate` lines, `w1.verdict` and the `OER-RESULT:` line, whose
  fail reasons each start with a gate's name (`walker-restart not
  exercised`), the block's `channel=` and masked `stand=` with the router
  AP's channel and masked BSSID, which catches a stale build, and
  `w1.config ssid_matches=` (report-only: how many BSSIDs carry the stand's
  SSID) with the `w1.bss` lines that carry `ssid_ok=1`. Exit 0 when both pass, 1 when both fail, 2 on
  the timeout and 3 when the block is missing or malformed or the image and
  the host disagree on anything. A summary follows: the gates as both see
  them and the beacon gaps that overlap a PHY tracking tick (the stand's
  smallest handoff lag maps the frame stamps onto the ticks' clock). Each
  `w1.bss` line also carries the stamp and handoff spans of its stamped
  frames (`stamp_span_us=`, `handoff_span_us=`): their ratio is the MAC
  stamp clock's rate. The `w1.lag` lines (at most 64, `i=` in order, the
  previous and next spacing checked against the stamps) and `w1.lagfit`
  are report-only: the host recomputes the fit (delay = lag − smallest lag;
  lower median, maximum, units handed off within `tolerance_us` after the
  next unit's stamp), prints it in the summary and treats an image fit it
  does not reproduce as a disagreement. So are `w1.tsf` (at most 64 stand
  beacons' TSF and handoff, `i=` in order) and `w1.tsffit`: the host
  recomputes the least-squares fit of handoff on TSF (residual less the
  smallest residual: lower median, p90, maximum; slope − 1 in ppm), checks
  `bss=` against the stand's `w1.bss idx=` and `n=` against the pairs.
- `monitor` opens the console without a reset: RTS is released before DTR.
- `boards` lists each attached board with the hub port found on the host and
  whether that port may be power-cycled (or why not).
- `power-cycle` switches the board's hub port off and on with `uhubctl`. The
  port is found on the host and switched only when the board is the one
  device below it and the stand file's hub policy allows it (the stand file,
  in `host/lab/router`). The board must leave USB while the port is off, or the
  port "did not cut power"; then it waits for the board to return.

## The stand router

Before every `run w1`, and on `cargo fw router` (without a board), the
stand's OpenWrt lab router is brought to W1's profile and verified by
`oer-lab-router` ([host/lab/router/README.md](../../lab/router/README.md) lists every script it
runs there); the profile stays, the earlier settings are not restored. Both
hold the router's lock, `$XDG_RUNTIME_DIR/open-esp-radio/routers/router-<hash>.lock`, for
the whole run. Only the masked BSSID (`xx:xx:xx:xx:F6:D0`), channel numbers
and UCI section names are printed; the SSID, the SSH target and the
interface never are, in no message, lock file or `log.txt`.
