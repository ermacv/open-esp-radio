# ESF alignment of a transmit MPDU

Facts of a stateless model of the vendor's `ieee80211_align_eb` leaf outside
this repository. Unverified: the model names the "pinned
`ieee80211_align_eb` leaf" without an evidence entry.

- **Admitted shapes:** the reservation in front of the data equals the
  802.11 header length, and only 24 (non-QoS) or 26 (QoS) bytes; anything
  else is refused.
- **Moved length:** header plus remaining payload, at most `0x3fff` (the
  descriptor's 14-bit length field).
- **Alignment:** the reserved start (data address minus the reservation) is
  rounded down to a 4-byte boundary, so at most three bytes move.
- **Storage word:** the descriptor word keeps its bits 28 and 13:0, sets bits
  31 and 30, and writes the moved length into bits 27:14:
  `(word & 0x1000_3fff) | 0xc000_0000 | length << 14`.

Bits 28, 30 and 31 of the transmit word have no evidence of their own.
