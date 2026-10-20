# IEEE 802.11 wire formats

`oer-ieee80211-wire`: allocation-free IEEE 802.11 frame formats, parsers, encoders
and shared values. Mutable exchange state belongs to upper-mac, station
and SoftMAC cores.

## Known defects

- **Channel 14 is admitted regardless of the regulatory domain and the PHY.**
  `channel.rs::WifiChannel::new_2_4_ghz` accepts channel 14 at 20 MHz, while
  the ESP32-S31 PHY rejects it (`PhyChipChannelRequest::validate`). A caller
  gets a channel the radio cannot tune and that most countries forbid.
  Channel 14 is legal only where the regulatory domain allows it and the PHY
  supports it.
- **A-MSDU subframes are parsed lazily** (`data.rs::AmsduSubframes`): a
  malformed later subframe surfaces only after earlier ones were yielded.
- **Raw header bits instead of types:** `ap.rs` encoders and
  `station/data.rs`, `station/data/amsdu.rs` set `header[1] |= 0x40`/`0x20`
  and `header[24] |= 0x80` although `FrameControl::with_protected` exists;
  `station/management.rs::StaProtectedManagementFrame::encode` ORs a raw
  `u16` and takes an unvalidated `pub ccmp_header: [u8; 8]`.
- **`ccmp.rs::ccmp_header`** is a public, unvalidated header builder with
  only test callers.
- **AP side** (`ap.rs::parse_ap_management_request`): Reassociation Request
  is not parsed, and the Listen Interval is read and dropped (see the AP
  crate's known defects).
- **No strict lint block:** `lib.rs` has only `forbid(unsafe_code)` and the
  crate has no `clippy.toml` (as `rsn` and `ap`).
