# IEEE 802.11 wire formats

`oer-ieee80211-wire`: allocation-free IEEE 802.11 frame formats, parsers, encoders
and the constants of the standard. Fixed-width fields use `oer-wire::bitfield!`
declarations with standard-clause sources; bounded readers and writers handle
byte streams. The crate holds no time, policy or state: it has no normal
dependency on `oer-time` or on an IEEE 802.11 crate above it
(`oer-test-layers`). Mutable exchange state, timing and selection policy belong
to upper-mac, the role cores and the runtime. The lower-MAC port's codecs live
here: the data and management MAC header (`frame::MacHeader`) with its QoS
Control field, the legacy rates of the Supported Rates elements (`rates`) and
the header geometry of a monitor-injected MPDU (`monitor_injection`).

Besides the frame bodies of the roles, the formats include the Frame Control
field with named management and control subtypes, the management MAC header
(`management::ManagementHeader`), Sequence Control access, the Block Ack
Action and Compressed BlockAckReq codecs, the Timestamp field and the time
unit (`tsf`), status and reason codes (`status`), EtherType values
(`ethertype`), the EAPOL-Key Key Information field and KDEs (`eapol`), the SAE
Commit and Confirm body codecs, whose scalar, element and confirm are byte
values for `oer-ieee80211-core-security-rsn` to interpret, and extension element IDs
(`sae`), the AAD and nonce of a CCMP-protected MPDU (`ccmp::protected`),
the BIP Management MIC element (`bip`), single information elements
(`element`), RSN element and RSNXE encoders, beacon template element edits and the HE-SIG-B user field codecs.
The chip's RX-report SIG-B streams live in the S31 MAC.

Production code forbids unsafe and denies every sans-IO panic lint, including
`disallowed_methods` and `disallowed_macros`. Its local Clippy configuration
retains the common workspace bans and the complete sans-IO catalogue. Tests
may assert; shared `test-support` builds keep the production code's policy.
Fixed fields retain array widths through compile-time offset admission.
Counted RSN lists retain complete borrowed byte spans and yield fixed arrays;
in-place moves admit both ranges before changing any byte.

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

Channels hold a one-byte index into a const table of admitted geometries;
frequency accessors and `RadioChannel::span` are total table reads. PHY rates
hold only their parameters; exhaustive const maps supply rates and N_DBPS. PPDU duration
calculation returns `None` when microseconds do not fit in `u32`; aggregate
admission refuses that candidate. Owned byte prefixes retain a `PrefixLength`
proof, and width-proved field setters do not revalidate encoder values.

Production code denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing and arithmetic-side-effect Clippy lints without local exceptions.
