# `cargo fw`

Builds the staged images of `firmware/apps`, writes them into a stand board's
flash and runs them to a verdict.

```console
cargo fw build <image>
cargo fw flash <image> --board <id|MAC>
cargo fw run <image> --board <id|MAC> [--timeout 30s]
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

## Boards

Boards and hubs come from the owner's stand file,
`~/.config/open-esp-radio/stand.toml` (or `OER_STAND_FILE`). Read are
`schema`, `[[hub]]`, `[[board]]`, `[station].ssid` and of
`[station_fixture]` `kind` (`open-wrt`), `ssh_target`,
`wireless_interface`, `radio` (the UCI `wifi-device` section), `ap_section`
(the `wifi-iface` section of the stand's AP) and `channel` (1 to 13), the
last group only for `run w1`. Nothing else is deserialized:
`[station].passphrase` and `[access_point]` are never read. Attached boards are found in sysfs as
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
  `Halted`), `walker_restart` (from `w1.hold`: a hold short of every
  staging slot is unexercised, `pool-not-exhausted`; units received while
  every lease was held fail it, `received-while-held`, as does
  `no-frames-after-release`; how the ring recovered, `w1.reload` and the
  walker snapshot `w1.exhaust`, is report-only), `irq_stack` (4096 bytes
  free), `service_errors` and `unserviced` (zero) and `timer_wake` (no
  timer wait of the image returned before its deadline, `woke_early=` of
  every `w1.timer` and `w1.wake` line; `woke-early`). It compares them with the
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
- `power-cycle` switches the board's hub port off and on with `uhubctl`, only
  when the stand file lists the port as switchable and not protected and the
  board's USB location in sysfs is that very port; it then waits for the
  board to return.

## The stand router

The stand's AP is a lab OpenWrt router: before every `run w1`, `cargo fw`
brings it to the W1 profile and verifies it (`src/router.rs`). The profile
is committed and stays; the router's earlier settings are not restored.
Each step is one `ssh -o BatchMode=yes -o ConnectTimeout=5
<ssh_target> <command>` (no prompt, at most 30 s); values reach the remote
shell single-quoted. In order:

```sh
uci changes wireless
uci show wireless
uci set 'wireless.<radio>.channel=<channel>' && uci set 'wireless.<radio>.htmode=HT20' && uci set 'wireless.<radio>.beacon_int=100' && uci set 'wireless.<ap_section>.ssid=<ssid>' && uci set 'wireless.<ap_section>.disabled=0' && uci commit wireless && wifi reload
uci show wireless
iw dev '<wireless_interface>' info (up to 16 times, 2 s apart)
```

1. Staged, uncommitted wireless changes refuse the run (the commit would
   take them along); they are named without their values.
2. Before anything is set: `<radio>` is an enabled 2.4 GHz `wifi-device`
   (`band='2g'`, or `hwmode` `11b`/`11g`), `<ap_section>` a `wifi-iface` of
   it in mode `ap`, not hidden, and no other enabled `wifi-iface` of the
   radio carries the stand's SSID (those are named by section and left as
   they are).
3. The profile: the stand's channel, HT 20 MHz, a 100 TU beacon interval,
   the stand's SSID, the AP section enabled. Encryption and key are never
   set or read.
4. The committed values are the profile's, and the stand's SSID is on
   exactly one enabled `wifi-iface` of the radio.
5. After the reload, `iw` is asked until the interface is an AP on the
   profile's channel, `width: 20 MHz`, with the stand's SSID: 30 s at most,
   then the run is refused with the last reason. Its `addr` is the stand's
   BSSID.

Of `uci show` only `device`, `mode`, `disabled`, `hidden`, `ssid`, `band`,
`hwmode`, `channel`, `htmode` and `beacon_int` are kept, an SSID only as
whether it is the stand's. The SSID, the SSH target, the interface and the
full BSSID are never printed: not on the console, in the lock file, in a
run's `log.txt`, in an error or a `Debug`; a remote command's output and
stderr are never shown (ssh's own errors name the host). Only the masked
BSSID (`xx:xx:xx:xx:F6:D0`), channel numbers and UCI section names appear.
