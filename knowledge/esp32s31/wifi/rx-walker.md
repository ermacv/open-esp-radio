# Wi-Fi RX descriptor walker: publication facts

Facts about how the ESP32-S31 Wi-Fi MAC's RX descriptor walker takes newly
published descriptors, and the design the `dma` crate's ring follows from
them (`crates/esp32s31/ieee80211/dma`, `rx_ring.rs`). Registers:
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

## Not verified

- A doorbell rung while NEXT is nonzero: whether it re-reads anything.
- What the walker does at NEXT = 0 beyond waiting (stop or retry), and what
  bit 27 changes.
- The meaning of NEXT bits 31:20.
- Other geometries (the product's 96 descriptors, other buffer sizes),
  other channels and bands.
- Which word0 bits the vendor sets or clears on detach or rearm.

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
