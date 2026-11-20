# The receive ring on the stand

`oer-hil-probes-esp32s31-rx-ring` runs the HAL's receive ring
(`radio::ieee80211::dma::receive`) on the product's path, from the root's
claim, over the radio domain it holds (`tasks/277`, `281`). The bring-up and
the ring's start and stop are the path table's, in `src/radio.rs` of the
sender (`esp32s31-mac-reset`), which this probe includes.

The ring:
- 16 descriptors and R, in an arena of 32 buffers of 256 octets with the
  64-octet header below the floor;
- the other 15 buffers are the spares a single is lent for;
- case C's 700- and 1 500-octet frames are chains of 3 and 7 descriptors.

A frame is the sender's when it is to the station, from the sender learned
first, with Frame Control one of the cases'. It is then checked:
- every byte the sender sets agrees (Duration, Sequence Control and Retry
  are the MAC's);
- its length agrees with its case's: the bytes after the header are the
  MPDU without its FCS, rounded up to a word (`HW_RX_CHAIN_LAPS_20261117`).

Once started it prints `MARK 6` and waits up to 120 s for the sender's
first unit (the `Waited` record); the cases begin with it, so the lease
starts the sender on that mark. Every window counts the sender's units and the ring's anomalies by kind
(`Length`, `DoorbellPending`). A window with no sender unit fails its case.

| Case | What | Pass |
|---|---|---|
| R1 singles | one 2 s window: every single lent for a spare (copied in place when none is held), its handle back to the spares | ≥ 1 000 sender units, no mismatch, no length disagreement, no anomaly, the 15 spares held at the end |
| R2 chains | the same window: every chain copied in place, its descriptors against its case's (`expected`); each continuation's control word recorded by position (first, middle) | ≥ 200 chains each of the 700 and 1 500 cases, none mismatched; the words are a record, no condition |
| R3 stall | 100 ms without serving, then the walker's standing (cursor, tail word, cursor), then a 1 s window | the cursor unchanged in one of the two recorded states (NEXT zero with the tail completed, or NEXT on the armed tail), which is recorded; ≥ 100 sender units after, no mismatch, no anomaly |
| R4 turns | 50 turns: a 200 ms window; the stop (the walker's edge, the stop request and its hold, the reset after a stop, the release); the cold start (preparation, channel, core, publication, walker); a 200 ms window | every turn: the 32 handles back, ≥ 50 sender units after the start and some before, no mismatch, no anomaly; the stop's and the start's microseconds recorded |

| Record | Values |
|---|---|
| 1 up | |
| 2 waited | the wait's milliseconds, the foreign units in it |
| 5 failed | phase, index |
| 20 window | case, sender units, singles, chains, mismatched, length disagreements, foreign units, `Length` anomalies, `DoorbellPending` anomalies, spares held |
| 21 chains | case tag, chains, descriptors least and most, expected, mismatched |
| 22 word | position (0 first, 1 middle), the control word, how many |
| 23 standing | state (0, 1, or 2 neither), LAST, NEXT (`u32::MAX` zero, `u32::MAX - 1` elsewhere), the tail, its completion, unchanged |
| 24 turn | index, sender units after, handles, stop µs, start µs, sender units before, mismatched after, `Length` and `DoorbellPending` anomalies |
| 25 verdict | case 1 to 4, 1 for its pass |

The console prints `MARK <n>` at each case (6 the wait for the sender), then one line a record, then
`OER-RESULT: pass rx-ring` when the four cases pass and nothing failed.

## Running

Build both images before the lease, from one commit:

```console
cargo fw build esp32s31-rx-ring --diagnostic
cargo fw build esp32s31-mac-reset --diagnostic
```

Three boards under one lease: s31-a receives, s31-b sends, and c5-a
teaches s31-b its peer (frames to s31-b's station from s31-a's address).

```console
cargo stand --owner wifi lease --board s31-a --board s31-b --board c5-a --air exclusive -- sh -c '
  cargo stand board send c5-a --timeout 30s SYNC "MODE 13 20 6m" &&
  { cargo fw run esp32s31-rx-ring --board s31-a --timeout 300s > rx.log &
    until grep -q "MARK 6" rx.log; do sleep 0.1; done
    cargo fw run esp32s31-mac-reset --board s31-b --timeout 400s > tx.log &
    until grep -q "MARK 70" tx.log; do sleep 0.1; done
    cargo stand board send c5-a --timeout 30s "TX 300 10000 512 <s31-b> <s31-a> <s31-a> 0"
    wait; }'
```

Every owner stays held until the leased host reset.
