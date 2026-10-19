# Wi-Fi HE A-MPDU: hardware HE-Control and Trigger MSDU lengths

Source: the ESP32-S31 A-MPDU formatter in
`chips/esp32s31/ieee80211/mac/src/tx/ampdu/` as it stood before its
host-only inline write path was removed, and the two tests that pinned these
facts there (`hardware_he_control_uses_vendor_metadata_bit_without_dma_placeholder`,
`he_trigger_preparation_uses_original_msdu_lengths_and_exact_link_range`).
The SOURCE tags are the ones those comments and tests carried. Facts only.

## The subframe DMA metadata prefix

Every A-MPDU subframe buffer starts with an eight-byte private metadata
prefix; the encoded 802.11 MPDU follows it at offset 8.

- Bytes 0..3: a little-endian word whose low fourteen bits are the PSDU
  length in bytes: the encoded MPDU, the hardware-generated MIC and the
  four-byte FCS.
- Byte 4: the number of empty delimiters after this subframe.
- Bytes 5..7: written zero by the product path, except for the bit below.

## Hardware HE-Control insertion

- Metadata byte 7 bit 0 (bit 24 of the second little-endian metadata word,
  `0x0100_0000`) set makes the MAC insert one four-byte HE-Control field into
  that MPDU on air.
- The DMA-resident frame does not change: no placeholder is reserved, the
  CCMP header stays immediately after the QoS header, and the low fourteen-bit
  PSDU length excludes the four inserted bytes.
- Length accounting adds `(byte7 & 1) * 4` bytes to the subframe, after its
  delimiter and its rounded length. The aggregate (APEP) length therefore
  includes the inserted bytes, and so does every APEP/TXOP admission check.
- Example: a 64-byte MPDU with an 8-byte MIC has PSDU length 76 in the
  metadata word; the one-subframe aggregate is 84 bytes (delimiter 4 + 76 +
  inserted 4).

SOURCE: complete `libpp[pp_he.o]::ppCalSubFrameLength` reads
`metadata[7] & 1`, multiplies it by four and adds it after the delimiter and
rounded metadata length; `HIL_VENDOR_HE_CONTROL_INSERTION_2026_07_30` captured
the vendor metadata word one as `0x0100_0000` while CCMP remained immediately
after the QoS header in DMA.

## Original MSDU lengths for Trigger-based publication

- Each MPDU of a Trigger-eligible HE aggregate carries the length of its
  original MSDU, before 802.11 encapsulation: neither the MPDU nor the PSDU
  length. The vendor keeps it as a 16-bit halfword in bytes, at `+0x22` of the
  frame state, named `msdu_len`. Zero means the value is not known.
- Trigger preparation sums the MSDU lengths of every MPDU in the linked
  aggregate. The sum is the queued-MSDU byte count, at most `0x000f_ffff`
  (20 bits), written to the queue's software buffer-status register
  (`WDEVTXQBSR_SW`, `wifi_mac_he_buffer_status.software_bsr(queue)` in the PAC)
  immediately before the validity read-modify-write that publishes the queue.
- The per-link MPDU-length entries of the same transition take the PSDU
  lengths (at most `0x3fff`), not the MSDU lengths.
- An MPDU without an MSDU length fails Trigger preparation closed: the
  aggregate cannot be published Trigger-eligible.
- Example: two MPDUs of 100 and 101 bytes (PSDUs 112 and 113) carrying MSDUs
  of 80 and 81 bytes queue 161 MSDU bytes. On the best-effort queue with TID
  limit `Three` and TID 0 the link reservation is queue 2, first link 0,
  count 2.

SOURCE: complete `libpp[hal_debug.o]::dbg_read_tx_ppdu` names frame-state
halfword `+0x22` `msdu_len`; complete `libpp[hal_mac_tx.o]::mac_tx_set_tb`
sums that halfword across the linked aggregate and publishes it to
`WDEVTXQBSR_SW`.

## State of the product path

- `RetainedDmaAmpduTx::commit_he` (through `commit_referenced_frame`) writes
  metadata bytes 5..7 as zero and records `hardware_he_control = false` and an
  MSDU length of 0 for every MPDU. HE-Control is never inserted, and an HE
  A-MPDU submitted with `HeAmpduTxConfig::with_trigger_based` fails with
  `HtAmpduTxError::TriggerMsduLengthUnavailable`: HE TB A-MPDU does not work on
  the chip.
- The rest of the path is in place: `HtAmpduTxStorage` keeps an MSDU length
  and a HE-Control flag per slot, retry compaction carries both, the length
  accumulator honours the HE-Control term, and the PAC programs the BSR.
- When a consumer of HE TB A-MPDU is connected, the commit input
  (`HeAmpduFrameRequest`) must carry the MSDU length and the HE-Control flag,
  and the commit must write them as described above.
