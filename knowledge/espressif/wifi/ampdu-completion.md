# Wi-Fi: A-MPDU completion in the vendor `libpp`

Source: a vendor comparison outside this repository of
`libpp.a[pp.o]::ppResortTxAMPDU` and the `lmac.o` retry path with the
production retained A-MPDU owner (`oer-esp32s31-ieee80211-mac`), pinned
ESP32-S31 archives (`knowledge/esp32s31/vendor/artifacts.toml`), last state
2026-10-17. Facts of the vendor code; the production policy values are in
`oer-espressif-ieee80211-policy` (`lmac`).

## Resort after a BlockAck

- The resort starts from the queue record `ppTxqUpdateBitmap` leaves: the
  BlockAck starting sequence and bitmap, the aggregate's `esf_buf` chain and
  each MPDU's eight-byte A-MPDU metadata. Metadata word 0 carries the PSDU
  length and, in bits 16..24, the low byte of the MPDU's sequence number,
  which the resort measures against the starting sequence.
- An MPDU kept for retry gets the Retry bit; an acknowledged one does not.
  Every-acknowledged, none, holes, a missing tail, a starting sequence beyond
  the window, wrap from 4095 to 0 and a single missing MPDU kept in the
  aggregate all behave as IEEE 802.11 expects and matched production.
- A missing MPDU stays in the aggregate without counting publications; only
  the MSDU lifetime bounds it (`lmacMSDUAged`, with the lifetimes `lmacInit`
  installs).
- When the BlockAck agreement is no longer operational
  (`trc_isTxAmpduOperational`, `trc_tid_isTxAmpduOperational`), the missing
  MPDUs go to ordinary transmission; a missing aggregate head is converted
  through `ppHEAMPDU2Normal`. The vendor sets their Retry bit in the resort.
- A Trigger-based success that ends an S-MPDU is resorted without reading a
  BlockAck and ends the aggregate with no Retry bit.

## BlockAckReq

- `ppFillAMPDUBar` / `ppReSendBar` request a BlockAck starting after the
  aggregate head when the resort discards the head as aged, or acknowledges
  it while the station has a request pending for the TID. Rate control sets
  that pending bit when it resumes aggregation (`trc_onAmpduOp`).
- The request is sent only while the aggregate keeps an MPDU after the
  resort; a later MPDU discarded as aged in the same resort cancels it.

## Completion without a BlockAck

- **CTS timeout:** no MPDU was sent; none gets the Retry bit.
- **ACK timeout:** every MPDU gets the Retry bit.
- Both end at the vendor's short retry limit; for the compared rate the rate
  record's publication limit does not undercut it. The vendor then ends the
  exchange and recycles the aggregate, except an RTS-protected aggregate whose
  CTS never arrived: `lmacEndRetryAMPDUFail` keeps it and sends a BlockAckReq
  starting at its head, whose BlockAck drives the ordinary resort.

## Where production differs (by design, at the last comparison)

- The MPDU immediately left of the starting sequence: production treats it as
  acknowledged, the vendor retransmits it.
- A single missing HT MPDU: production hands it to its ordinary retry owner.
- MSDU lifetime: production starts it when the aggregate is committed, the
  vendor at the `pp` queue enqueue timestamp. Under software queueing delay a
  production MSDU is discarded later than the vendor's by that delay.
- After a lost BlockAck agreement production sets the Retry bit on the copy it
  republishes rather than in the resort.
- Metadata bits 16..24: production writes zero. Which vendor code writes the
  sequence low byte, and whether hardware reads it, is not established.

At that comparison production sent no BlockAckReq at all. The carried MAC
crate now has a `RequestBlockAck` decision for the CTS-failure case; whether
it also covers the aged-head and pending-request cases was not re-checked.
