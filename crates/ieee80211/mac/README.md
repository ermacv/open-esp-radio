# IEEE 802.11 MAC types

`oer-ieee80211-mac`: allocation-free IEEE 802.11 protocol types and state
machines.

## Known defects

Found in the predecessor's review of 2026-10-16 and still present.

- **Fragment PNs only have to increase.**
  `fragmentation/reassembly.rs::append_fragment` refuses only
  `observed <= previous`; IEEE 802.11-2020 §12.5.3.4.4 and Linux require
  `previous + 1` (the FragAttacks mixed-key defence). The fragment identity
  (`fragmentation.rs::OpenDataFragmentIdentity`) is bound to the wire KeyID,
  not to the key-installation epoch (`ingest` passes epoch 0).
- **Public PN and replay hazards** (`ccmp.rs`, `ccmp/replay.rs`):
  `CcmpTxPacketNumber::continuing_after` is public with only test callers;
  `CcmpTxPacketNumber::new` is not tied to a key; `CcmpRxReplayEpoch::install_group`
  and `install_group_epoch` are public and overwrite an active replay
  frontier (a KRACK-class reinstall). Production uses `exchange_group`.
- **A-MSDU subframes are parsed lazily** (`data.rs::AmsduSubframes`): a
  malformed later subframe surfaces only after earlier ones were yielded.
- **Raw header bits instead of types:** `ap.rs` encoders and
  `station/data.rs`, `station/data/amsdu.rs` set `header[1] |= 0x40`/`0x20`
  and `header[24] |= 0x80` although `FrameControl::with_protected` exists;
  `station/management.rs::StaProtectedManagementFrame::encode` ORs a raw
  `u16` and takes an unvalidated `pub ccmp_header: [u8; 8]`.
- **Untyped TID:** `ccmp.rs::CcmpReplayError::InvalidTid` exists because the
  TID is a raw value.
- **`ccmp.rs::ccmp_header`** is a public, unvalidated header builder with
  only test callers.
- **Low:** `fragmentation/reassembly.rs::preflight_in_epoch` checks
  `CONTEXTS == 0` at run time (`NoReassemblyContexts`) instead of a const
  assert; `ccmp.rs` has `impl Default for CcmpRxReplayState`.
- **AP side** (`ap.rs::parse_ap_management_request`): Reassociation Request
  is not parsed, and the Listen Interval is read and dropped (see the AP
  crate's known defects).
- **No strict lint block:** `lib.rs` has only `forbid(unsafe_code)` and the
  crate has no `clippy.toml` (as `rsn` and `ap`).
