# RSN security

`oer-ieee80211-rsn` holds the allocation-free, hardware-independent RSN (IEEE 802.11
robust security network) protocol shared by WPA2 and WPA3 suites. It is
sans-IO and owns no executor, timer, key slot or transmit path: chip role
crates execute its typed key-data unwrap, transmit and key-install requests.

| Module | Responsibility |
| --- | --- |
| `akm` | Key descriptor version, PTK expansion and EAPOL-Key MIC of each `oer_ieee80211_wire::security::rsn::Akm` |
| `sae` | SAE commit/confirm, hunting-and-pecking and hash-to-element password elements, anti-clogging tokens |
| `bip`, `management_ccmp` | BIP-CMAC-128 for group-addressed and CCMP for individually addressed robust management frames |
| `ccmp_mpdu` | Software CCMP for wire data and management MPDUs, using one immutable association SPP decision |
| `element` | Association RSN element policy and selection of its first supported suite; wire syntax is `oer_ieee80211_wire::security::rsn` |
| `crypto` | Zeroizing PMK/PTK owners, PSK passphrase derivation and the association security binding |
| `eapol` | Validated borrowed/owned EAPOL-Key packets and MIC verification |
| `frames` | EAPOL-Key/Ethernet construction, GTK key data and owned security elements |
| `aes` | RFC 3394 key wrap and unwrap and the asynchronous unwrap port |
| `state` | Station and authenticator four-way-handshake transitions |
| `supplicant` | Station orchestration: typed key-data unwrap and key-install requests and their completions, response deadlines |
| `keys`, `retry` | Owned CCMP key installs and event-driven EAPOL retransmission |

A `Ptk` records the suite that derived it, so MIC generation and verification
cannot mix suites. The authenticator takes its suite from the station's
validated Association element; the supplicant takes it from its own
Association element. Both reject EAPOL-Key frames whose descriptor version
differs from that suite.

`oer_ieee80211_wire::ccmp::Ccmp128Key` owns temporal-key bytes, redacts Debug
output and zeroizes its storage on drop. RSN copies directly from borrowed,
zeroizing PTK/GTK storage into this owner; lower-MAC installation moves the
same type without extracting an intermediate array. PTK derivation writes
directly into the zeroizing PTK, and GTK parsing borrows the key-data buffer
into a zeroizing GTK. Pending actions, PMF reception and retained station
records use the common owner; adapters and comparisons borrow its bytes.
This covers those owners, not arbitrary compiler temporaries.

`CcmpAssociation` authenticates the QoS A-MSDU-present bit in both AAD and
nonce flags when SPP was negotiated. Without SPP it masks that bit according
to ordinary CCMP. Management MPDUs retain their subtype in AAD and use the
management nonce flag, independently of SPP. The model applies this transform
to transmitted wire bytes and verifies incoming encrypted fixtures; a failed
MIC erases the tentative plaintext before returning the error. Packet-number
allocation and replay admission remain with their separate installed owners.
`ManagementCcmpReceiver` adds its management PN frontier around that same
transform, so a rejected MIC erases the tentative body on the station's PMF
path too. The CCM authenticator accepts only the closed `Aad` type built from
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


`BipTransmitter` refuses a frame when its 48-bit IPN space is exhausted, or
when the output has no room for the MMIE; refusal consumes no IPN and leaves
the frame unchanged. The IGTK epoch's owner retains this state across an AP
restart or channel move rather than reusing IPNs under the same key.

The implemented suites are PSK (`00-0F-AC:2`), PSK-SHA256 (`00-0F-AC:6`) and
SAE (`00-0F-AC:8`). The MAC package names them and the negotiated
association (`oer_ieee80211_wire::security::AssociationSecurity`); this crate
owns only their cryptography, each property an exhaustive match over the
suite.

The strict-lint contract requires boundary constructors to retain checked
fixed-size fields as array references obtained with `split_first_chunk`.
Variable-length envelopes must establish each boundary before those fields
reach crypto and handshake transitions. Runtime expect/unwrap paths become
retained type proofs or typed peer-input refusals, without mechanically
replacing indexing with `.get()` or suppressing the lint. The crate root does
not enable the policy.

## Known defects

- **The authenticator reuses the replay counter on M1/M3 retries.**
  `state/authenticator.rs::RsnApState::retry_transmit` resends the stored
  counter, and M2/M4 match one stored value, not a window of sent ones.
  wpa_supplicant drops an M3 whose counter is not greater than the last one,
  so a lost M4 ends the handshake. Our supplicant hides it:
  `supplicant.rs::RsnConnectedSupplicant::on_message3` and
  `state/supplicant.rs` accept an equal counter as a retransmission.
- **AES key schedules are not wiped.** `Cargo.toml` enables `aes` without
  its `zeroize` feature; no test checks `Aes128: ZeroizeOnDrop`. The
  HMAC/SHA-2 state holding the KCK and PMK has no wipe either.
- **SAE H2E ignores the Rejected Groups salt.** `sae.rs::SaeCommit::process`
  always derives the keyseed with a zero salt; `rejected_groups` is only
  parsed. H2E with a rejected group does not interoperate.
- **SAE secrets are copyable or printable** (`sae.rs`):
  `SaePasswordElement` and `SaePasswordToken` derive `Clone, Copy, Debug`;
  `SaeKeys` is `Clone` with `pub kck`, `pmk`, `pmkid`; `SaePassword`
  is `Clone`; `SaeComebackTokens.key` is never zeroized.
- **Management CCMP receiver** (`management_ccmp.rs::ManagementCcmpReceiver`):
  starts at no PN, so PN 0 is accepted; the KeyID is not checked to be 0;
  A1 is not checked to be individually addressed. `new` copies the TK from
  a raw `&[u8; 16]`.
- **Raw keys in the API:** `ccmp_mpdu.rs::CcmpAssociation::seal`/`open`
  take `&[u8; 16]` instead of `&Ccmp128Key`.
- **Self-generated CCMP fixtures only** (`ccmp_mpdu/test_vector.rs`): no
  IEEE 802.11-2020 Annex J CCMP vector, no hostap `wlantest` QoS or
  4-address vector.
- **Non-constant-time comparison** of IGTK material in
  `bip.rs::BipReceiver::rekey`; `akm.rs` still builds the KCK MAC with
  `new_from_slice(..).expect`.
- **No strict lint block:** `lib.rs` has only `forbid(unsafe_code)` and
  the crate has no `clippy.toml` (as `mac` and `ap`).
