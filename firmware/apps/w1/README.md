# w1

A passive scan on the product receive path, which measures the receive
runner's provisional constants (`IMMEDIATE_CONTINUATIONS`,
`CONTINUATION_WAIT`, `MASK_CONFIRM_TIMEOUT`, `DRAIN_TIMEOUT`, `DRAIN_POLL`,
`ACTOR_STEP_BOUND`) and checks that the stand's access point is heard.

The radio system brings Wi-Fi up as in W0, on the configured channel. The
receive runner (`oer-esp32s31-ieee80211::rx::receive`) binds its arena on the
radio executor (P2), polled in the handler of `FROM_CPU` line 2; the MAC and
WDEVPWR interrupts and the radio doorbell (`FROM_CPU` line 3) run the
interrupt line at P3. Staged frames reach the application (thread mode, P1)
through the SPSC queue; it normalizes each and, in the window, hands it to
the survey (`oer-ieee80211-runtime::survey`).

| Phase | What happens |
| --- | --- |
| bring-up | join, track, MAC cold start; the runner binds and starts |
| first frame | at most 5 s for the first staged frame |
| window | 10 s from the first frame: every frame to the survey |
| stress | 5 s: a P2 timer every 37 µs, a P1 timer every 113 µs |
| hold | every staging lease held (at most 3 s to collect), 1 s, release; frames counted for 2 s |
| stop | stop request, frames drained until the exit |

After the exit the image prints a block from `OER-W1-BEGIN v=1` to
`OER-W1-END lines=<n>` (its format is checked by `cargo fw`,
`tools/fw/src/w1.rs`), then `OER-RESULT: pass w1 beacons=<N> required=<R>`
or `OER-RESULT: fail w1 <gate> <reason>[; …]`. The gates: the stand's
beacons in the window (N = beacons − duplicates, at least
⌊8·window / (10·interval)⌋; the stand is the BSSID of the stand router's
AP interface, and its stored SSID must be the stand's: heard with another
SSID it fails `stand-mismatch`, not heard `stand-absent`, however many other
BSSIDs carry the SSID), the applied channel, a clean exit, the walker's
restart (every staging lease held, no unit received during the 1 s hold,
frames after the release; `w1.hold`), at least 4096 free bytes of the
interrupt stack, no service error, no MAC or WDEVPWR entry without the line,
and no timer wait of the image (P1, and P2's stress task) returning before
its deadline (`waits=`/`woke_early=` in `w1.timer` and `w1.wake`). A
failure before the runner hands over its ends prints only
`OER-RESULT: fail w1 <reason>`.

Report-only: `w1.config ssid_matches=`, how many BSSIDs carry the stand's
SSID (each `w1.bss` line says whether it does, `ssid_ok=`); `w1.exhaust`, the walker registers (NEXT with its unknown
upper flag, LAST, BASE, the walker enable, the reload doorbell, the
starvation counter) and the ring's view (base, recycle frontier, accepted
tail, observed and completed descriptors, the list left from NEXT, a
pending reload, released, detached and staged buffers) that the runner read
when a lease returned to a full staging pool (every slot claimed or every
buffer detached, as during the hold), before the capacity drain serviced
anything, for the longest such stretch; `w1.reload`, how the ring
recovered; per BSS the stamp and handoff spans of its stamped frames, whose
ratio is the MAC stamp clock's rate; and `w1.lag`, the receive stamp and
handoff lag (handoff low 32 bits less stamp) of the first 64 stamped units
the survey took, with the spacing to the previous and next unit's stamp.
`w1.lagfit` summarizes them: the stamp clock runs at the handoff clock's
rate, so the smallest lag stands for the clocks' offset and a unit's
handoff delay is its lag less the smallest lag (taken relative to the first
lag, so a wrap is harmless); it prints the lower median and the maximum
delay and how many units were handed off 0 to 2000 µs after the next
unit's stamp, both on the stamp clock: handed off only once the next unit
arrived.

The MAC stamp repeats across frames, so `w1.tsf` measures the handoff
against the access point's own clock instead: the TSF and handoff (µs, low
32 bits) of the first 64 beacons of the stand BSS (its survey index is
`bss=`; no stand, no pair) that the survey counted in the window, each with
a new TSF. `w1.tsffit` fits handoff on TSF by least squares in integer
arithmetic (both relative to the first pair), floors each residual to µs
and prints the lower median, the 90th percentile (rank ⌈0.9·n⌉) and the
maximum of residual less the smallest residual, and `slope_ppm`, the slope
less one in ppm (floored); `-` below two pairs.

```sh
cargo fw run w1 --board s31-a
```

`cargo fw run w1` first brings the stand router to the W1 profile (the
stand file's channel, HT 20 MHz, 100 TU beacons, the stand's SSID;
`tools/lab/README.md`) and reads its AP's BSSID and channel. They and the
SSID reach the image as build-time values (`OER_W1_CHANNEL`,
`OER_W1_BSSID`, `OER_W1_SSID_HEX`) that `build.rs` writes to
`$OUT_DIR/w1_config.rs`; built without them (`cargo fw build w1`) the image
prints `OER-RESULT: fail w1 unconfigured`. BSSIDs are printed with only
their last two octets, SSIDs only as their length and `ssid_ok=`, and the
base MAC never.

Nothing is transmitted. eFuse is only read, as in W0.
