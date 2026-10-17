# IEEE 802.11 protocol boundaries

These packages contain portable MAC mechanisms, role policy, service contracts
and security. Hardware representation and radio execution live below and above
them respectively; they are not dependencies of the portable protocol policy.

Within the MAC package, formats and retained state are separated inside the
protocol that owns them:

| Module under `mac/src/` | Responsibility |
| --- | --- |
| `block_ack/frame` | Stateless Block Ack Action parsing and wire identifiers |
| `block_ack/session` | One TX agreement, its negotiation generation and alarm handling |
| `block_ack/originator` | One peer's TX agreements, at most `TIDS`: a Dialog Token sequence they share, and each TID's queued negotiation with its attempts and the retry interval (`TxBlockAckRetry`) after a request that did not leave or a response that did not come; the station and the access point use it |
| `block_ack/reorder` | One receive agreement's bounded reorder window and a complete slot release for each input |
| `fragmentation` | Validated fragment identities and the shared fragment contract |
| `fragmentation/parsing` | Header/body validation without reassembly ownership |
| `fragmentation/reassembly` | Complete bounded reassembler, slots and admission tokens |
| `ap/profile` | Explicit advertisement values; the chip AP profile selects rates, capabilities and WMM parameters |
| `station/association` | Association capability types, validation and encoding |
| `station/management` | Probe/authentication management codecs |
| `ap` | AP management/data codecs; plaintext and pairwise-protected management headers and request bodies, without authenticating RX |
| `security` | Link protection, station and access point security policies, and the negotiated `AssociationSecurity` both roles meet at |
| `security/rsn` | RSN element wire syntax and the `Akm` suite vocabulary shared by station selection and the RSN crate |
| `station/security` | Station RSN candidate policy and the selected association element |
| `station/data` | Data codecs, with A-MSDU framing in `data/amsdu` |
| `sequence` | `SequenceNumber`: the twelve-bit value and its modulo-4096 window arithmetic |
| `station/sequence` | Separate management/non-QoS and per-TID TX sequence owners |
| `data/duplicate` | Association/peer-scoped receive retry history |
| `qos` | Typed traffic intent, UP/AC and DSCP classification helpers |
| `channel` | `Channel` (2.4 GHz and 5 GHz, 20/40 MHz geometry of the global operating classes) and the 2.4 GHz `WifiChannel` the role owners configure |
| `channel_switch` | `ChannelSwitch`: the Channel Switch and Extended Channel Switch Announcements in beacons and action frames, their target `Channel`, and their encoding; the switch's timing belongs to the caller |
| `phy` | `PhyRate`: validated non-HT, HT and HE SU rates |
| `extensions/wmm` | WMM AC parameters and vendor IE parsing |
| `extensions/espressif/esp_now` | ESP-NOW v1/v2 framing and protected-envelope validation |
| `extensions/espressif/esp_now/v2/reassembly` | Caller-owned storage for a validated v2 datagram |

The public Block Ack, fragmentation, station and data namespaces expose their
protocol contracts. Every frame builder, sequence owner and reorder window
takes `sequence::SequenceNumber`, so twelve-bit range validation happens once
at construction and raw register fields convert at the chip boundary.
Consumers use `qos` for traffic intent, `extensions::wmm` for WMM elements and
`extensions::espressif::esp_now` for vendor MAC framing.

A receive reorder window always waits at a vacant head between inputs. At
most N-1 frames are retained, so a release of N positions holds them and the
one received frame. Each old frame transfers to its own result position;
the vacant head reserves a result position for that input. No fallible
append follows the state change. Physical positions are relative to the
window head: `sequence % capacity` would alias distinct sequences around
4095→0 when the capacity does not divide 4096. Immediate delivery performs
the same slot-ownership check as the buffered path.

QoS classification includes the DSCP mapping and downgrade helpers. The
admission/downgrade loop belongs to the chip MAC TX runtime; parsing an
advertised WMM Parameter Set neither acquires admission nor selects a
hardware queue. AP encoders take an explicit `Advertisement` and a
`Channel` of either band: a 5 GHz beacon or Association Response carries
no DSSS Parameter Set and no ERP element, and a beacon refuses a 5 GHz
advertisement with a DSSS rate. The portable codec carries no implicit
ESP32-S31 profile; a chip AP selects its own advertisement.

`arbiter/src/coordinator.rs` decides the channel of a port a station and an
access point share (`ChannelCoordinator`): the station's BSS decides it, and
the access point follows within its `ApFollowPolicy` (2.4 GHz alone by
default, never a channel that needs radar detection; `ApUnservable` says
whether the access point stops or the station leaves where it may not
follow). It turns the upstream's announced switch into the access point's own
announcement, counted in the beacons that fit before the upstream's instant,
with one retune of the port at the access point's switch, and searches for an
upstream lost without an announcement in short absences after the access
point's TBTTs, densely and then sparsely (`ApSearchPolicy`). It takes facts
and returns `CoordinatorAction`s; the port's owner carries them out.

[`lower-mac`](lower-mac/README.md) declares the radio port every Wi-Fi backend
implements; one submission is one hardware transmission attempt. Optional
features are its extension traits. An attempt carries no backoff: a backend
draws its queue's EDCA contention below the port, with
`softmac/src/edca.rs`. [`upper-mac`](upper-mac/README.md) decides
everything else above the port for a transmission: the retry ladder and
Retry bit, A-MPDU retry selection, protection, and the planner that turns an
exchange into attempts; `mac/src/block_ack/reorder.rs` reorders received
Block Ack sessions and `mac/src/ccmp.rs` checks receive replay; its transmit
packet-number allocator serves the backend, which numbers protected MPDUs
below the port. Vendor limits, ladders and estimates for these
algorithms are parameters; the Espressif ones live in
[`oer-espressif-ieee80211-policy`](../espressif/ieee80211-policy/README.md).

`softmac/src/contract` describes operation ownership, service capabilities,
resource limits and normalized statuses. Configuration, VIF and monitor
contracts remain explicit sibling modules. ESP-NOW peer/protocol owners live
in `softmac/src/extensions/espressif/esp_now/protocol`; secrets, peer generations
and replay state live in its `security` sibling. They use lower MAC codecs.
The MAC package must not depend back on SoftMAC peer or security policy.

The [RSN crate](rsn/README.md) separates secret ownership and
derivation (`crypto`), packet views and MIC handling (`eapol`), wire formats
(`frames/{security_ies,key_data,transmit}`) and the complete handshake owners
(`state/{supplicant,authenticator}`).

Portable AP `service` retains one peer storage owner and separates `peer`,
`security`, `block_ack` and `power_save` operations into child modules.
Chip AP `engine` retains the hardware/service/key/beacon owner and separates
`management`, `tx`, `rx` and `power_save` operations without duplicating state.

Module namespaces do not create independent owners. Tests live in child files
beside their owner or at the shared protocol boundary. See the
[architecture](../../docs/architecture.md#layers) for the complete layering
contract.
