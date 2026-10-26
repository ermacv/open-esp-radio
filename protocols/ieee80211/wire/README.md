# IEEE 802.11 wire formats

`oer-ieee80211-wire`: allocation-free IEEE 802.11 frame formats, parsers, encoders
and shared values. Fixed-width fields use `oer-wire::bitfield!` declarations
with standard-clause sources; bounded readers and writers handle byte streams.
Mutable exchange state belongs to upper-mac, station
and SoftMAC cores.

Production code denies `unwrap_used`, `expect_used`, `panic`, `unreachable`
and `indexing_slicing`; unit tests use the `cfg(test)` exception. Narrow
`#[expect(..., reason = "...")]` attributes document validated internal
invariants. Shared `test-support` library builds retain the production policy.

## Known defects

- **Channel 14 is admitted regardless of the regulatory domain and the PHY.**
  `channel.rs::WifiChannel::new_2_4_ghz` accepts channel 14 at 20 MHz, while
  the ESP32-S31 PHY rejects it (`PhyChipChannelRequest::validate`). A caller
  gets a channel the radio cannot tune and that most countries forbid.
  Channel 14 is legal only where the regulatory domain allows it and the PHY
  supports it.
- **A-MSDU subframes are parsed lazily** (`data.rs::AmsduSubframes`): a
  malformed later subframe surfaces only after earlier ones were yielded.
- **`ccmp.rs::ccmp_header`** is a public, unvalidated header builder with
  only test callers.
- **AP side** (`ap.rs::parse_ap_management_request`): Reassociation Request
  is not parsed, and the Listen Interval is read and dropped (see the AP
  crate's known defects).

Channel constructors admit and retain the regulatory span and primary/center
frequencies; their accessors and `RadioChannel::span` are total. PHY rate
constructors retain their exact rates and symbol fractions. PPDU duration
calculation returns `None` when microseconds do not fit in `u32`; aggregate
admission refuses that candidate. Owned byte prefixes retain a `PrefixLength`
proof, and width-proved field setters do not revalidate encoder values.

Production code denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing and arithmetic-side-effect Clippy lints without local exceptions.
