# RSN security

`oer-ieee80211-rsn` holds the allocation-free, hardware-independent RSN (IEEE 802.11
robust security network) protocol shared by WPA2 and WPA3 suites. It is
sans-IO and owns no executor, timer, key slot or transmit path: chip role
crates execute its typed key-data unwrap, transmit and key-install requests.

| Module | Responsibility |
| --- | --- |
| `akm` | Key descriptor version, PTK expansion and EAPOL-Key MIC of each `oer_ieee80211_wire::security::rsn::Akm` |
| `sae` | SAE commit/confirm cryptography, hunting-and-pecking and hash-to-element password elements, the validity of a peer's scalar and element, anti-clogging tokens; the Commit and Confirm body codecs are `oer_ieee80211_wire::sae` |
| `bip`, `management_ccmp` | BIP IPN allocation and replay, and the management CCMP receiver over the stateless transforms in `oer-ieee80211-cipher`; the management receiver's PN frontier is `oer_ieee80211_upper_mac::ccmp::CcmpReplayFrontier`; their receive errors add `Replay` to the transform's refusals |
| `element` | Association RSN element policy and selection of its first supported suite; wire syntax is `oer_ieee80211_wire::security::rsn` |
| `crypto` | Zeroizing PMK/PTK owners, PSK passphrase derivation and the association security binding |
| `nonce` | Boot-seeded ChaCha20 owner for fresh supplicant nonces |
| `eapol` | Validated borrowed/owned EAPOL-Key packets and MIC verification |
| `frames` | EAPOL-Key/Ethernet construction, GTK key data and owned security elements; the Key Information field, KDEs, SAE element IDs and status codes are `oer-ieee80211-wire`'s |
| `state` | Station and authenticator four-way-handshake transitions |
| `supplicant` | Station orchestration: typed key-data unwrap and key-install requests and their completions, response deadlines; each key-data request yields an owned `oer_crypto::KeyUnwrapRequest` (`RsnKeyUnwrapRequest`) |
| `authenticator` | Access point orchestration: PTK derivation from the association's PMK, Message 2 and 4 MIC verification, the binding of the station's security IEs and the Message 3 wrap and MIC over key data the access point supplies; `authenticator::sae` is the access point's SAE responder |
| `pmksa` | The PMKSA cache of both roles: per peer and, for a station, per SSID |
| `keys`, `retry` | Owned CCMP key installs and event-driven EAPOL retransmission |

`oer-ieee80211-cipher` owns software CCMP, BIP-CMAC-128 and the software
AES-128 block primitive with its key-unwrap engine; the RFC 3394 algorithm is
`oer-crypto`'s. Key data is bounded by `RSN_KEY_DATA_CAPACITY`. It has no
handshake state, replay frontier or clock.

The crate is Apache-2.0: `authenticator/sae.rs` ports the ESP-IDF access
point's SAE handling (`NOTICE`).

`PmkDerivation::new` validates and owns the password/SSID; `step` evaluates
one HMAC-SHA1 and yields the PMK exactly once after 8192 steps (two blocks,
4096 iterations each). The composition schedules between steps. Completion,
explicit zeroization and drop erase the pending password and partial key state;
there is no synchronous passphrase-derivation API. The synchronous `pbkdf2`
dependency is used only by tests as an independent comparison. `SNonceSource`
consumes one 256-bit entropy fill and advances a separate ChaCha20 stream;
both cipher state and buffered keystream zeroize on drop.

A `Ptk` records the suite that derived it, so MIC generation and verification
cannot mix suites. The authenticator takes its suite from the station's
validated Association element; the supplicant takes it from its own
Association element. Both reject EAPOL-Key frames whose descriptor version
differs from that suite.

`oer_ieee80211_contracts_security::Ccmp128Key` owns temporal-key bytes, redacts Debug
output and zeroizes its storage on drop. RSN copies directly from borrowed,
zeroizing PTK/GTK storage into this owner; lower-MAC installation moves the
same type without extracting an intermediate array. PTK derivation writes
directly into the zeroizing PTK, and GTK parsing borrows the key-data buffer
into a zeroizing GTK. Pending actions, PMF reception and retained station
records use the common owner; adapters and comparisons borrow its bytes.
This covers those owners, not arbitrary compiler temporaries. AES-128/192/256
key schedules enable RustCrypto's `zeroize` feature and wipe on drop, including
when the stateless cipher crate is built independently. HMAC and SHA-1/SHA-2
internal state derived from KCK/PMK material does not wipe on drop; zeroizing
the input owners does not erase that state. SAE passwords, password elements,
password tokens, derived keys and comeback-token keys zeroize on drop. Their
owners do not implement `Copy` or `Clone`, and Debug reveals only a redaction.
Derived keys expose borrowed bytes through accessors.

`oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation` authenticates the QoS A-MSDU-present bit in both AAD and
nonce flags when SPP was negotiated. Without SPP it masks that bit according
to ordinary CCMP. Management MPDUs retain their subtype in AAD and use the
management nonce flag, independently of SPP. The model applies this transform
to transmitted wire bytes and verifies incoming encrypted fixtures; a failed
MIC erases the tentative plaintext before returning the error. Packet-number
allocation and replay admission remain with their separate installed owners.
`ManagementCcmpReceiver` starts its management PN frontier at zero, rejects
PN zero and admits only KeyID zero with an individually addressed A1 around
that same transform, so a rejected MIC erases the tentative body on the station's PMF
path too. Its constructor moves a `Ccmp128Key`; CCMP transforms borrow that
typed owner instead of accepting raw key arrays. The CCM authenticator accepts only the closed `Aad` type built from
a decoded MAC header: its four layouts occupy 22, 24, 28 or 30 bytes. It
cannot accept an arbitrary slice whose suffix would escape authentication.

GTK KDE parsing retains `GroupKeyId` in `RsnGtk` and `RsnKeyKind::Group`;
IDs outside the two-bit wire domain never reach a key installation. The
recorded M3 regression in `frames/captured_m3.rs` authenticates and unwraps
a PSK-SHA256 + MFP Message 3 that hostapd sent on a hardware bench. Derived
KDE cases change only its ID to exercise all four wire values, including zero.

The portable station can retain a completed key installation while waiting for
M4 delivery. `RsnStaSupplicant` authenticates a repeated M3 before advancing its
counter. The station retains the installed PTK and processes only changed GTK
material from a newer M3; an exact repeat produces M4 without an install.
`RsnStaReady::RetryInstall` carries `RsnStaRetryInstall`, which has no pairwise
field. `RsnConnectedSupplicant::on_message3` authenticates exact and newer
repeats with the retained PTK and produces only M4 or a group unwrap request.
The request binds the authenticated M3, including its full ANonce; its GTK/IGTK
completion cannot install PTK. Unwrap completion yields `RsnStaReady`, a type
without another KEK request, so callers exhaustively handle all remaining work.
A connected unwrap request owns its authenticated frame and a zeroizing KEK
copy. It has no busy latch or wrapping ticket: validation of a successor can
proceed while the station holds the current TX. Unwrap and install completions
check the committed replay frontier, so an overtaken request cannot roll it
back. The station owns the bounded deferred-frame policy and TX ordering.


`BipTransmitter` retains the last protected `BipPacketNumber`, a distinct
48-bit domain from a CCMP PN. It returns `BipTransmitError::Exhausted` when
no larger IPN remains (the IGTK must be replaced), or `InvalidFrame` for an
invalid length or insufficient output capacity. Refusal consumes no IPN and
leaves the frame unchanged. `BipReceiver` rejects repeats and rollbacks and
commits its frontier only after MIC verification; repeated delivery of the
same IGTK can only raise that frontier. Both use upper-MAC's typed
`packet_number` mechanism, shared with CCMP. The IPN frontiers are public
values and are not zeroized; IGTK material still zeroizes on drop. The IGTK
epoch's owner retains its transmit state across an AP restart or channel
move rather than reusing IPNs under the same key.

IEEE Std 802.11-2020 9.4.2.54 defines the MME's six-octet IPN;
12.5.4.4-.6 require strictly increasing 48-bit IPNs and rejection of a
received IPN at or below the frontier.

The implemented suites are PSK (`00-0F-AC:2`), PSK-SHA256 (`00-0F-AC:6`) and
SAE (`00-0F-AC:8`). The MAC package names them and the negotiated
association (`oer_ieee80211_wire::security::AssociationSecurity`); this crate
owns only their cryptography, each property an exhaustive match over the
suite.

The crate forbids unsafe code and denies the complete sans-IO panic lint
catalogue. Fixed-size key blocks and validated borrowed fields retain their
array types; owned variable-length bytes use zeroizing bounded vectors.
Public frame/element views return their typed parse errors instead of relying
on a successful earlier parse. All callers propagate or handle these errors.

SAE field and point conversions retain `CtOption` validity through both H2E
maps, inversions, square roots and constant-time coordinate selection. Only
the final invariant validity is converted to `SaeError::Crypto` at the public
operation boundary; intermediate values never cause a secret-dependent early
return. Commit secrets retain validated scalar types. Point conversion uses
`DecompressPoint` and checked field coordinates without the SEC1 tag parser.
Encoding a commit reconstructs y with a field square root, adding one exponentiation.
The comeback table selects and updates its slot with `ConditionallySelectable`.
Tests retain local allowances for assertions and fixture construction. The
`test-support` feature exposes owned-buffer invalidation only for caller
regressions; production owners have no such mutator.

## Known defects

- **The authenticator reuses the replay counter on M1/M3 retries.**
  `state/authenticator.rs::RsnApState::retry_transmit` resends the stored
  counter, and M2/M4 match one stored value, not a window of sent ones.
  wpa_supplicant drops an M3 whose counter is not greater than the last one,
  so a lost M4 ends the handshake. Our supplicant hides it:
  `supplicant.rs::RsnConnectedSupplicant::on_message3` and
  `state/supplicant.rs` accept an equal counter as a retransmission.
- **SAE H2E ignores the Rejected Groups salt.** `sae.rs::SaeCommit::process`
  always derives the keyseed with a zero salt; `rejected_groups` is only
  parsed. H2E with a rejected group does not interoperate.
- **The SAE responder can be exhausted with spoofed MACs.**
  `authenticator/sae.rs::receive_commit` allocates a session before checking
  status, token or commit; sessions have no expiry or retransmission timer.
  No access point uses the responder yet.
- **Non-constant-time comparison** of IGTK material in
  `bip.rs::BipReceiver::rekey`.
