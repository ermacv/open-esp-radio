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
| hold | two cycles (`forward`, then `reverse`): every staging lease held (within the fill window, below), 1 s, a staged release (one drop, 1.5 s dwell, the rest 10 ms apart); frames counted for 2 s |
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
restart (in the first cycle a full hold, no unit received during the 1 s
hold, frames after the release; `w1.hold cycle=0`), the
ownership experiment (`w1.own`, below), at least 4096 free bytes of the
interrupt stack, no service error, no MAC or WDEVPWR entry without the line,
no timer wait of the image (P1, and P2's stress task) returning before
its deadline (`waits=`/`woke_early=` in `w1.timer` and `w1.wake`), one
`RX_DESCRIPTOR_BASE` write in the epoch (`base_writes`: the cold
publication of the reserved descriptor; `w1.reload base_writes=`), and at
most one frame landing in the reserved descriptor, the epoch's first,
with none after the walker reached the ring (`reserved`;
`reserved_landings=` ≤ 1, `reserved_late_landings=` 0). A
failure before the runner hands over its ends prints only
`OER-RESULT: fail w1 <reason>`.

A hold is full (`w1.hold full=1`) with every staging lease held
(`reason=all`), or stuck one short (`reason=stuck-at-last`): 31 leases,
and the runner's latest drain end (`WifiRxReport::drain_end`, printed as
`end_*=`) shows 31 buffers staged and detached, at most one released
(the buffer without a lease is the walker's frozen LAST unit, still owned
by the ring, or released but not rearmable while it is LAST), all 32
descriptors observed, NEXT zero (no armed descriptor) and LAST the
descriptor without a lease, and no unit received since the last lease for the
fill window (one lease short, the fill waits that long after the last
lease). Otherwise `full=0 reason=-`.

The fill window (`fill_ms=`) is max(3 s, (32 + 4) beacon intervals of the
stand), at most 10 s, with the interval from the survey (`bi_tu` × 1024 µs);
6 s when the survey has no beacon interval of the stand. On a quiet channel
the stand's beacons are most of the frames: on channel 13, about 10 frames
per second, a fixed 3 s fill collected 29 of 32 leases and left the walker
restart and the ownership experiment unexercised. At the usual 100 TU the
window is 3.69 s.

```text
w1.hold cycle=<0|1> fill_ms=<3000..10000> reached=<n> slots=32 full=<0|1> reason=all|stuck-at-last|- units=<n> mac_irqs=<n> stage_capacity_blocks=<n> admission_blocks=<n> end_released=<n|-> end_detached=<n|-> end_staged=<n|-> end_observed=<n|-> end_recycle_start=<n|-> end_accepted_tail=<n|-> end_next=<0xhex|-> end_last=<0xhex|->
```

The ownership experiment asks whether the walker writes into a buffer the
application holds. Each cycle fingerprints every lease it takes: a CRC-32
of the frame bytes the lease shows (`segment().buffer`, offset 0 to the
frame's length, each byte read volatile), the length and the descriptor
index, `(descriptor_address − ring_base) / 12` with `ring_base` the DMA
address of descriptor 0 that the arena's bind laid out (the ring prepared
from it has the same base, `w1.exhaust ring_base=`). At the end of the
1 s hold, before any drop, every fingerprint is rechecked
(`hold_mismatch=`). When the leases cover the 32 descriptors once (31 of
them, for a stuck hold: the buffer at LAST has no lease and is skipped),
the first lease taken stands for the recycle frontier and the last for
the accepted tail: `forward` drops from the first lease taken in ring
order (the tail's lease last), `reverse` from the last one against ring
order (`first_drop=`, `-` when the leases did not cover the ring once:
then they drop in the order they were taken, or its reverse). After the
first drop the image takes no frame for 1.5 s (a frame the walker stages
meanwhile stays queued, so no append to an empty list moves BASE), then
rechecks the rest (`dwell_mismatch=`); every later lease is rechecked just
before its own drop (`release_mismatch=`, the first drop's recheck
included), 10 ms apart. `first_mismatch=` is the first changed lease's
descriptor index (`x` unknown, `-` none). Then, for 2 s, the descriptor
indices of the first 16 frames taken (`first_after_release=`, `x` outside
the ring, `-` none), the pairs among them not consecutive in the ring
(`order_breaks=`, recomputed by `cargo fw` from the list and the
`arena=` size), and the chained and oversized drops from the first drop
on (`chained=`, `oversized=`). `discards=` lists every nonzero drop or
refusal count over the same window as `<reason>:<n>` (`-` none): the
runner's drops `empty`, `too_long`, `chained`, `oversized`, `overload`,
whose unit never reaches the application (its index is missing from the
list), then the application's RX-control prefix refusals `rx_failure`,
`mic_failure`, `quarantined`, `invalid`, `prefix_chain`, `bounds`,
`output_small`, `metadata`, `ignored`, `unsupported`, whose frame was
taken (its index is listed). `cargo fw` sets the descriptors the list
skips against the runner's drops and names the skips no drop counts. The `ownership` gate fails, `foreign-write`,
on any changed lease or chained drop in either cycle; otherwise a cycle
whose hold was not full leaves it unexercised (`pool-not-exhausted`), as does
one whose leases did not cover the ring (`ring-order`). The second cycle
starts after the first one's 2 s of frames, once the ring has recovered.

```text
w1.own cycle=<0|1> order=forward|reverse held=<n> first_drop=<idx|-> hold_mismatch=<n> dwell_mismatch=<n> release_mismatch=<n> first_mismatch=<idx|x|-> first_after_release=<i,i,…|-> order_breaks=<n> chained=<n> oversized=<n> discards=<reason>:<n>,…|-
```

Report-only: `w1.config ssid_matches=`, how many BSSIDs carry the stand's
SSID (each `w1.bss` line says whether it does, `ssid_ok=`); `w1.exhaust`, one line per cycle (`cycle=`), the walker registers (NEXT with its unknown
upper flag, LAST, BASE, the walker enable, the reload doorbell, the
starvation counter) and the ring's view (base, recycle frontier, accepted
tail, observed and completed descriptors, the list left from NEXT, a
pending reload, released, detached and staged buffers) that the runner read
when a lease returned to a full staging pool (every slot claimed or every
buffer detached, as during the hold), before the capacity drain serviced
anything: the latest snapshot at the end of the cycle's dwell, when its
first drop took one (`snapshots=` is the run's count then). It also
carries what the capacity drain's first service pass read when it
completed a reload: `next_after_doorbell=`, NEXT right after the doorbell
cleared, and `last_after_doorbell=`, LAST, which that read takes only
when NEXT is zero (`-` without a reload in that pass); and
`desc_hash_changed=`, whether a hash of every descriptor word changed
between the first drain that left the pool full and the snapshot. `w1.reload`, how the ring
published and recovered: the most doorbell observations one reload took
(`max_attempts=`, `limit=`), the reload settles' NEXT/LAST observations,
the doorbells rung once more because the walker had exhausted the old list
(`reload_rerings=`) and those after which it still had not resumed
(`rering_failures=`, a fault), and the reserved descriptor's gated counts
(`base_writes=`, `reserved_landings=`, `reserved_late_landings=`); per BSS the stamp and handoff spans of its stamped frames, whose
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
cargo fw run w1 --board s31-a --timeout 60s
```

The two hold cycles take 10 to 19 s, which makes a run longer than the
default 30 s timeout.

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
