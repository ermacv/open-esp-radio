# Portable Wi-Fi access point

`oer-ieee80211-ap`: executor- and chip-independent Wi-Fi access-point MLME.
No other crate uses it yet; its public calls are reached only from its own
tests.

Runtime panic lints and the complete sans-IO catalogue are denied at the
crate root. Service construction, authentication and removal report
`ApServiceError::GenerationExhausted` before reusing an identity generation;
failed authentication or removal retains the live peer and its security
state.
Initialized beacon lengths are validated, and advertisement access returns
its typed length error if the stored invariant is broken.

The security an access point offers (`security_policy.rs::ApSecurityPolicy`)
is this crate's policy: its link and management protection and the RSN
element and RSNXE it advertises, which `oer-ieee80211-wire` encodes into
beacons and responses as `BssSecurityElements`. The TBTT schedule
(`tbtt.rs::next_tbtt`) and the beacon's channel-switch announcements, edited
in the template through the wire's beacon element edits, are its own too.
Status codes and SAE body fields are the wire's. The four-way handshake of
each peer is an `oer_ieee80211_rsn::authenticator::RsnAuthenticator`, which
this crate drives with the association's PMK and Message 3's key data; the SAE
responder and the PMKSA cache are `oer-ieee80211-rsn`'s too. Transmit sequence
spaces and the management replay frontier are `oer-ieee80211-upper-mac`'s.

An access point uses 40 MHz in 2.4 GHz only under the 20/40 coexistence rules;
a station reports 20/40 OBSS as the standard requires.

## Known defects

**Security**

- **M1/M3 retries reuse the replay counter.**
  `service/security.rs::message3_transmit` rebuilds the same frame through
  `rsn`'s `RsnAuthenticator::message3`, and `service/tests.rs` asserts
  identical retry bytes. A lost M4 ends the handshake with wpa_supplicant.
- **The M3 Key RSC is all zero** (`rsn`'s `authenticator.rs`
  `complete_message3` and `message3`); it must carry the GTK's current TX
  PN, as the IGTK IPN already does. A station may accept replayed group
  frames.
- **BIP does not guard +HTC** (`service/management.rs::protect_group_management`).

**Timing and protocol**

- **Beacon timing from executor time.** `beacon.rs::ApBeacon::prepare`
  stamps `now` as the TSF and starts the TBTT grid at the first `prepare`;
  `wire`'s `beacon.rs::stamp` and this crate's `tbtt.rs::next_tbtt` follow it. TBTT is not
  at TSF mod BI×1024 = 0 and DTIM jitters around BI boundaries.
- **No reassociation.** `mac`'s parser drops Reassociation Request, and
  `service/peer.rs` `associate_rsn`/`associate_open` refuse an Association
  Request from an authorized peer (`WrongPeerPhase`) instead of resetting
  it and running a new 4-way.
- **PMF:** `service/peer.rs::authenticate_open` accepts Open System on a
  WPA3 BSS (and resets an existing PMF peer); plaintext Deauthentication/
  Disassociation reaches `remove_peer`; no AP-initiated SA Query, no status
  30 with a Timeout Interval; `service/management.rs` `receive_management`
  and `management_tx_protection` have no caller.
- **Untyped DTIM grant:** `service/power_save.rs::begin_buffered_group_release`
  returns `Option` over a bare `u16` count; `beacon/publication.rs::ApDtimRelease`
  is not consumed per frame.

**Medium and low**

- `service.rs` `ApBufferedGroupRelease`/`ApBufferedUnicastRelease` lack
  `#[must_use]` and a stop rollback (`discard_buffered_groups` refuses).
- `ApPeerBinding` takes the global generation, separate from
  `ApAssociationIdentity`.
- Listen Interval is discarded: no PS aging, no status 51, no filtered-frame
  requeue, no Null for an empty PS-Poll; no class-2/3 Deauthentication
  (reason 6/7); no null-data poll before `service/peer.rs::begin_due_peer_close`.
- CSA: `ApBeacon::prepare` decrements the count per prepared beacon, and
  `announce_channel_switch`/`channel_switch_due` have no consumer.
- BIP lives in `AccessPointService.group_management: Option<BipTransmitter>`
  outside the WPA3 security variant; group protection checks that both
  owners are present.
- `service.rs::ApPeer` holds flags and `Option`s beside its `phase`;
  `ApMlmeAction` is one enum for unrelated calls; `new_ap_tx_block_ack` is
  not `const`; `power_save.rs::complete_buffered_group` is unused;
  a misplaced doc on `restart_schedule`; `status_revision` is bumped per
  frame; no TX aggregation on an Open BSS; no GTK rekey; `Cargo.toml`
  still says "single-peer".
