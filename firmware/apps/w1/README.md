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
⌊8·window / (10·interval)⌋), the applied channel, a clean exit, the walker's
restart after the hold, at least 4096 free bytes of the interrupt stack, no
service error and no MAC or WDEVPWR entry without the line. A failure before
the runner hands over its ends prints only `OER-RESULT: fail w1 <reason>`.

```sh
cargo fw run w1 --board s31-a --expect-bssid 02:00:00:00:F6:D0 --channel 6
```

The BSSID and channel reach the image as build-time values
(`OER_W1_EXPECT_BSSID`, `OER_W1_CHANNEL`) that `build.rs` writes to
`$OUT_DIR/w1_config.rs`; built without them the image prints
`OER-RESULT: fail w1 unconfigured`. BSSIDs are printed with only their last
two octets, SSIDs only as their length, and the base MAC never.

Nothing is transmitted. eFuse is only read, as in W0.
