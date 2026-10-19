# Wi-Fi RX descriptor walker: publication facts

Facts about how the ESP32-S31 Wi-Fi MAC's RX descriptor walker takes newly
published descriptors, and the design the HAL's Wi-Fi DMA ring follows from
them (`chips/esp32s31/hal/src/ieee80211/dma`, `rx_ring.rs`). Registers:
`RX_DESCRIPTOR_BASE` (BASE), `RX_NEXT_DESCRIPTOR` (NEXT, low 20 address bits
and an opaque upper field), `RX_LAST_DESCRIPTOR` (LAST, low 20 bits),
`RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` (the reload doorbell, bit 0) and
`RX_CONTROL.WALKER_ENABLE`.

Sources:

- `HIL_W1_RX_WALKER_2026_10_18`: W1 stand runs 1–6 of 2026-10-18, the open
  driver on the W1 image (`firmware/apps/w1`): a 32-descriptor ring of
  1664-byte buffers, 32 staging leases, 2.4 GHz, the stand's channel; two
  hold cycles per run (every lease held until the walker ran out, then a
  forward or reverse release). The console logs stay in the ignored
  `target/fw/runs/` directories.
- `HIL_OPEN_RX_LIVE_APPEND_2026_07_27`: a 32-entry rotating ring, more than
  7000 receives on the open driver.
- Vendor: `BLOB_LIBPP_MAC_RXBUF_INIT_PREFIX`, `ROM_REV0_WDEV_APPEND_RX_BLOCKS`
  (`rom[]::wDev_AppendRxBlocks`), `ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR`
  (`rom[]::hal_mac_rx_get_last_dscr`), `BLOB_LIBPP_HAL_INIT_TAIL`, and the
  register model (`registers/esp32s31`).

## Observed on hardware

**H1: a doorbell rung with NEXT = 0 resumes the walker at `LAST.next`.**
With the walker exhausted (NEXT = 0, LAST = the accepted tail), the driver
rearmed a returned prefix, linked it through the old tail's `next` word and
rang the reload doorbell. When the doorbell cleared, NEXT read exactly
`LAST.next`, the first rearmed descriptor (the ring's recycle start), and
frames followed. The NEXT read after the doorbell, as descriptor indices, in
the cycles the stand lead listed: 29, 19, 25, 26, 2, 3, and so on for the
remaining cycles of runs 1–6, each equal to that cycle's recycle start.
SOURCE: HIL_W1_RX_WALKER_2026_10_18 (`w1.exhaust next_after_doorbell=`,
`recycle_start=`).

**H2: a bare BASE write with NEXT = 0 is picked up.** When every buffer
returned before any rearm (the reverse release), the driver of that date
published the whole returned list by writing BASE directly, with no
doorbell; the walker fetched the new head. Runs 4 and 6 (reverse full
cycles): `exhausted_republications=1 republications_accepted=1`. SOURCE:
HIL_W1_RX_WALKER_2026_10_18 (`w1.reload`).

**No foreign write into held buffers.** In 10 full hold cycles no lease the
application held changed while held, during the dwell or before its drop
(`w1.own hold_mismatch=0 dwell_mismatch=0 release_mismatch=0`). SOURCE:
HIL_W1_RX_WALKER_2026_10_18.

**Exhaustion snapshots.** The H1 values are the exhaustion snapshots'
reads: the walker registers and the ring's view taken when a lease returned
to a full pool, before the capacity drain serviced anything, and the NEXT
the drain's first reload settle read right after the doorbell cleared
(`w1.exhaust`, its fields described in `firmware/apps/w1/README.md`). The
per-cycle snapshot values are in the run logs. SOURCE:
HIL_W1_RX_WALKER_2026_10_18.

A rotating 32-entry ring published only through the doorbell received more
than 7000 frames. SOURCE: HIL_OPEN_RX_LIVE_APPEND_2026_07_27.

## Vendor behaviour

- BASE is written in three places: the cold `mac_rxbuf_init`
  (SOURCE: BLOB_LIBPP_MAC_RXBUF_INIT_PREFIX); the empty-software-list branch
  of `wDev_AppendRxBlocks`, which publishes the returned chain's head; and
  the reload suffix of the same function: after the bounded wait
  (`0x186a1` observations) for `RX_CONTROL` bit 0 to clear, a whole-word
  NEXT of zero makes it read LAST, and when LAST is not the new tail it
  writes BASE = `LAST->next`. SOURCE: ROM_REV0_WDEV_APPEND_RX_BLOCKS.
- Order: the new head is stored through the old tail's `next` word before
  the doorbell (`hal_mac_rx_set_dscr_reload`) is rung; hardware clears the
  doorbell after accepting it. SOURCE: ROM_REV0_WDEV_APPEND_RX_BLOCKS.
- On rearm the vendor writes `0xdeadbeef` at the buffer and at
  `buffer + capacity`. SOURCE: ROM_REV0_WDEV_APPEND_RX_BLOCKS.
- A walk is delimited by terminal bit 30 through the saved LAST, which
  `hal_mac_rx_get_last_dscr` reads. SOURCE: ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR.
- `RX_CONTROL` bit 27 is set once, by
  `hal_mac_set_rxbuf_reload_use_hw_beacon_enable` in the `hal_init` tail,
  and never cleared by any known writer. Its meaning is not established
  (the register model marks it opaque). SOURCE: BLOB_LIBPP_HAL_INIT_TAIL.
- The `BUFFER_FULL` starvation counter (`WIFI_MAC_RX_STATISTICS`, bits 15:0)
  is read-only; no vendor reaction to it is known.

## Owner bit (word0 bit 31)

Source: an experiment on ESP32-S31 rev0 (board s31-a), receive-only on
channel 13 with the stand AP, ring of 32 descriptors × 1664 bytes plus the
reserved descriptor, 2026-10-18; six runs, every W1 gate passing in the
last three. SOURCE[HIL_W1_OWNER_BIT_2026_10_18]. The experiment code was
never merged; the method below is complete without it.

**Method.** All access on P2 between service passes; the probed descriptor
P is excluded from every take while probed.

1. P = NEXT + 3, 5 or 7, and it must be armed (word0 exactly
   `0x81a00680`: bit 31 set, bits 30/29 clear, size = length = 1664),
   short of the accepted tail, not LAST, not NEXT, ring-owned (not leased),
   and the walker past the reserved descriptor with no reload in flight.
2. P's whole buffer (1668 bytes, guard included) is filled with a seeded
   pattern, device fence, read back for a CRC-32.
3. NEXT is re-read (still short of P), then word0 = `0x01a00680` (armed
   with bit 31 cleared, nothing else changed), device fence.
4. Observation every 2 ms for 2 s: LAST, NEXT, P's word0, the descriptors
   after P, the buffer CRC.
5. Recovery: word0 = `0x81a00680` again, device fence, one reload doorbell
   (fence, request, fence, wait until it clears); a second doorbell only if
   nothing lands within 500 ms.

For LAST: in a full hold (all 32 leases held, NEXT = 0, LAST = the accepted
tail, its word0 completed), bit 31 of LAST's word0 is cleared, every other
bit kept (`0xc0430680` → `0x40430680` in these runs), device fence; then
the normal release rearms the group behind LAST and rings the doorbell.

**Results.**

| Series | Repeats | Result |
|---|---|---|
| bit 31 = 0 on an armed descriptor ahead of the walker | 12 (3 in the first three runs, 9 in the last three; positions 3, 5, 7 ahead) | **The walker stops at it**: NEXT = P, LAST = P − 1, for the whole 2 s; P's buffer unchanged (CRC), its word0 untouched (bit 30 = 0, length 1664), no descriptor after P completes; frames on air are lost meanwhile (`buffer_full` +390..1124 over 2 s) |
| bit 31 restored, one doorbell | 9 | **The walker resumes at P** with no repair: the next frame lands in P (CRC changed, bit 30 set), then P+1, P+2, … in ring order; 0 extra doorbells; 360–405 ms until the next frame on the quiet channel |
| bit 31 = 0 on LAST at exhaustion, then rearm behind it and doorbell | 6 (both cycles of three runs, full holds, LAST a leased descriptor) | **H1 holds**: NEXT after the doorbell is the rearmed head, frames continue in order; the walker does not check LAST's own owner bit; 0 extra doorbells |

The held leases' CRCs stayed unchanged in every hold cycle of these runs.

**So:** the S31 walker honours the owner bit of the descriptor it is about
to fill: with bit 31 clear it stops there without writing, and a doorbell
after restoring the bit resumes it in place. It does not consult the owner
bit of the descriptor it last completed (LAST) when a doorbell asks it to
follow LAST's link.

## Not verified

- A doorbell rung while NEXT is nonzero: whether it re-reads anything.
- What the walker does at NEXT = 0 beyond waiting (stop or retry), and what
  bit 27 changes.
- The meaning of NEXT bits 31:20.
- Other geometries (the product's 96 descriptors, other buffer sizes),
  other channels and bands.
- Which word0 bits the vendor sets or clears on detach or rearm.
- The owner bit beyond the states above: on a descriptor the walker reaches
  through BASE, on the reserved descriptor, at other geometries, and
  whether the stop lasts indefinitely (observed for 2 s).

## The design that follows

BASE names one reserved descriptor R outside the rotation, contiguous with
the ring (inside the bound DMA range and the descriptor window), with its own
buffer. The cold publication is the epoch's only BASE write: BASE = R,
`R.next` = the ring head, R armed. Every later publication links the
accepted tail and rings the doorbell, whose resumption of an exhausted
walker H1 shows.

- **Why R.** H2 shows that a bare BASE write also works, but BASE is
  then a second publication path whose ordering against the doorbell and
  the walker's own fetches is unknown, and the vendor's reload suffix writes
  BASE from a LAST that may be stale. With BASE fixed on R, no ring
  descriptor is ever published through BASE, so no stale BASE can send the
  walker into a descriptor software already owns.
- **R's link.** The first ring take of the epoch proves the walker left R
  (it reached the ring through `R.next` or a resumed NEXT), and clears
  `R.next`: R becomes a dead end. A frame that lands in R before that take,
  normally the epoch's first frame, is counted and dropped uncopied; a write
  into R after it fails closed.
- **The whole ring cannot follow its own tail.** When every descriptor
  returned at once, the group stops one short: all but the accepted tail are
  rearmed and linked behind it, and the doorbell resumes the walker at their
  head; the old tail rejoins once the walker has moved past it. A unit
  spanning the whole ring cannot be a frame for the product geometries
  (`(COUNT − 1) × BUFFER` exceeds the largest encoded MPDU length) and fails
  closed.
- **The vendor race.** A settle that reads NEXT = 0 with LAST short of the
  new tail rings the doorbell once more instead of writing BASE; NEXT still
  zero after that contradicts H1 and fails closed.
- **The second layer: the owner bit.** R keeps the walker out of software's
  descriptors by construction; the owner bit (word0 bit 31, `OWNER`) stops
  it should that construction ever fail (SOURCE:
  HIL_W1_OWNER_BIT_2026_10_18, "Owner bit"). When a buffer leaves the ring
  into a lease, its descriptor's word0 is rewritten to its completed image
  with bit 31 clear (`RX_DONE`, length and size kept) and fenced, before the
  lease is handed out; the descriptor at LAST too, since a doorbell with
  NEXT = 0 still follows `LAST.next`. The rearm sets the bit again. A NEXT
  that names a detached descriptor (observed, bit 31 clear) means the
  walker stopped on that barrier: the reload settle after a doorbell and
  each drain's frozen LAST/NEXT read check for it, count it
  (`walker_on_detached`) and fail closed. The W1 image checks every held
  lease's descriptor for bit 31 clear (`w1.own owner_clear=`) and gates
  both.
