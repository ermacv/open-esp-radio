# Portable Wi-Fi access point

`oer-ieee80211-ap`: executor- and chip-independent Wi-Fi access-point MLME.
No other crate uses it yet; its public calls are reached only from its own
tests.

## Known defects

**Security**

- **M1/M3 retries reuse the replay counter.**
  `service/security.rs::build_pending_transmit` rebuilds the same frame
  from `rsn`'s `RsnApState`, and `service/tests.rs` asserts identical retry
  bytes. A lost M4 ends the handshake with wpa_supplicant.
- **The M3 Key RSC is all zero** (`service/security.rs` `complete_message2`
  and `build_pending_transmit`); it must carry the GTK's current TX PN, as
  the IGTK IPN already does. A station may accept replayed group frames.
- **BIP does not guard +HTC** (`service/management.rs::protect_group_management`).
- **No strict lint block** in `lib.rs`, no `clippy.toml`.

**Timing and protocol**

- **Beacon timing from executor time.** `beacon.rs::ApBeacon::prepare`
  stamps `now` as the TSF and starts the TBTT grid at the first `prepare`;
  `mac`'s `beacon.rs::stamp` and `tbtt.rs::next_tbtt` follow it. TBTT is not
  at TSF mod BI×1024 = 0 and DTIM jitters around BI boundaries.
- **`service/block_ack.rs::take_tx_block_ack_offer` panics** (`expect`) on
  an unknown peer of a protected BSS.
- **No reassociation.** `mac`'s parser drops Reassociation Request, and
  `service/peer.rs` `associate_rsn`/`associate_open` refuse an Association
  Request from an authorized peer (`WrongPeerPhase`) instead of resetting
  it and running a new 4-way.
- **PMF:** `service/peer.rs::authenticate_open` accepts Open System on a
  WPA3 BSS (and resets an existing PMF peer); plaintext Deauthentication/
  Disassociation reaches `remove_peer`; no AP-initiated SA Query, no status
  30 with a Timeout Interval; `service/management.rs` `receive_management`
  and `management_tx_protection` have no caller.
- **The SAE responder can be exhausted with spoofed MACs.**
  `sae.rs::receive_commit` allocates a session before checking status,
  token or commit; sessions have no expiry or retransmission timer and are
  a phase plus `Option` fields behind eight `expect`s.
- **Untyped DTIM grant:** `service/power_save.rs::begin_buffered_group_release`
  returns `Option` over a bare `u16` count; `beacon/publication.rs::ApDtimRelease`
  is not consumed per frame.

**Medium and low**

- `service.rs` `ApBufferedGroupRelease`/`ApBufferedUnicastRelease` lack
  `#[must_use]` and a stop rollback (`discard_buffered_groups` refuses).
- `service.rs` `with_security`/`advance_peer_generation`: the `u32`
  generation overflows into an `expect`; `ApPeerBinding` takes the global
  generation, separate from `ApAssociationIdentity`.
- Listen Interval is discarded: no PS aging, no status 51, no filtered-frame
  requeue, no Null for an empty PS-Poll; no class-2/3 Deauthentication
  (reason 6/7); no null-data poll before `service/peer.rs::begin_due_peer_close`.
- CSA: `ApBeacon::prepare` decrements the count per prepared beacon, and
  `announce_channel_switch`/`channel_switch_due` have no consumer.
- BIP lives in `AccessPointService.group_management: Option<BipTransmitter>`
  outside the WPA3 security variant, so `protect_group_management` and
  `message3_key_data` need `unreachable!`/`expect`.
- `service.rs::ApPeer` holds flags and `Option`s beside its `phase`;
  `ApMlmeAction` is one enum for unrelated calls; `new_ap_tx_block_ack` is
  not `const`; `power_save.rs::complete_buffered_group` is unused;
  `beacon.rs::ApBeacon::from_initialized` does not validate `len`;
  a misplaced doc on `restart_schedule`; `status_revision` is bumped per
  frame; no TX aggregation on an Open BSS; no GTK rekey; `Cargo.toml`
  still says "single-peer".
