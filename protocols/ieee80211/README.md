# IEEE 802.11 protocol boundaries

These packages contain portable wire formats, MAC mechanisms, role policy,
service contracts and security. Chip drivers implement the lower-MAC port;
the portable runtime drives role services over that port.

| Module relative to this directory | Responsibility |
| --- | --- |
| `wire/src/block_ack/frame.rs` | Block Ack Action (ADDBA, DELBA) and Compressed BlockAckReq codecs |
| `upper-mac/src/block_ack/session.rs` | One TX agreement, negotiation generation and alarms |
| `upper-mac/src/block_ack/originator.rs` | Per-peer/TID TX agreements, shared Dialog Token sequence and negotiation retries |
| `upper-mac/src/block_ack/reorder.rs` | One receive agreement's bounded reorder window and slot release |
| `wire/src/fragmentation/parsing.rs` | Fragment header/body validation and its frame refusals |
| `upper-mac/src/fragmentation/reassembly.rs` | Bounded reassembly, slots, admission tokens and reassembly refusals |
| `wire/src/ap/profile.rs` | Explicit AP advertisement values |
| `wire/src/station/association.rs` | Association capability types, validation and encoding |
| `wire/src/station/management.rs` | Probe/authentication management codecs |
| `wire/src/ap.rs` | AP management/data codecs and protected management envelopes |
| `wire/src/security.rs` | Link protection, negotiated association security and the elements a BSS advertises |
| `wire/src/security/rsn.rs` | RSN element and RSNXE syntax, encoders and AKM suite vocabulary |
| `wire/src/station/security.rs` | The RSN element and RSNXE of a station's association request |
| `wire/src/eapol.rs` | EAPOL-Key Key Information field and Key Data Encapsulations |
| `wire/src/sae.rs` | SAE Authentication body fields and SAE extension element IDs |
| `wire/src/status.rs` | Status and reason codes |
| `wire/src/ethertype.rs` | EtherType values |
| `wire/src/tsf.rs` | The Timestamp field and the time unit |
| `sta/src/security.rs` | Station security policy: candidate admission and RSN selection |
| `ap/src/security_policy.rs` | Access point security policy and its advertised elements |
| `wire/src/station/data.rs` | Data codecs, with A-MSDU framing in `station/data/amsdu.rs` |
| `wire/src/sequence.rs` | Twelve-bit sequence numbers and modulo-4096 arithmetic |
| `sta/src/sequence.rs` | Separate management/non-QoS and per-TID TX sequence owners |
| `upper-mac/src/duplicate.rs` | Peer-scoped receive retry history |
| `wire/src/qos.rs` | User priorities, access categories and the network header fields carrying traffic intent |
| `upper-mac/src/qos.rs` | Ethernet traffic classification, DSCP mapping and admission downgrade |
| `runtime/src/vif.rs` | Receive routing for one same-channel STA plus SoftAP pair |
| `wire/src/channel.rs` | Validated 2.4/5 GHz channels and 20/40 MHz geometry |
| `wire/src/channel_switch.rs` | Channel Switch Announcement parsing and encoding |
| `wire/src/phy.rs` | Validated non-HT, HT and HE SU rates |
| `wire/src/rates.rs` | Legacy rates of the (Extended) Supported Rates elements |
| `wire/src/frame.rs` | Frame Control with named subtypes, the data/management MAC header codec, Sequence Control access and the QoS Control field |
| `wire/src/management.rs` | The management MAC header, Capability Information and probe requests |
| `wire/src/beacon.rs` | Beacon encoding, DTIM/TIM stamping and template element edits |
| `wire/src/monitor_injection.rs` | Header geometry of a monitor-injected MPDU |
| `wire/src/extensions/wmm.rs` | WMM AC parameters and vendor IE parsing |
| `wire/src/extensions/espressif/esp_now.rs` | ESP-NOW framing and protected-envelope validation |
| `softmac/src/extensions/espressif/esp_now/reassembly.rs` | Caller-owned ESP-NOW v2 datagram storage |

Frame builders, sequence owners and reorder windows use
`wire::sequence::SequenceNumber`, validating the twelve-bit range at
construction. Raw register fields convert at the chip boundary.

A receive reorder window waits at a vacant head between inputs. At most N-1
frames are retained, so a release of N positions holds them and the received
frame. Each old frame transfers to its own result position; the vacant head
reserves a result position for the input. No fallible append follows the
state change. Physical positions are relative to the window head:
`sequence % capacity` would alias distinct sequences around 4095→0 when the
capacity does not divide 4096. Immediate delivery checks the same slot
ownership as the buffered path.

The upper MAC's QoS module classifies traffic and downgrades access
categories over the wire's priority values. The caller
supplies traffic intent to the port; the backend lowers its checked access
category to a hardware queue. Parsing a WMM Parameter Set acquires no
admission. AP encoders take an explicit advertisement and channel: a 5 GHz
beacon or Association Response carries neither a DSSS Parameter Set nor ERP,
and a beacon refuses a 5 GHz advertisement containing a DSSS rate. Chip AP
profiles supply their own advertisement values.

[`arbiter`](arbiter/src/coordinator.rs) decides the channel of a port shared
by a station and an access point. The upstream BSS chooses it; the AP follows
within `ApFollowPolicy`, or `ApUnservable` selects which role stops. The
coordinator turns upstream channel switches into AP announcements and one
port retune. Its search policy schedules upstream searches between AP TBTTs.
It takes facts and returns actions for the port owner to execute.

[`port`](port/README.md) holds only the peripheral boundary: lifecycle, VIF
configuration, the monitor tap, operation ownership, capabilities, the
backend's service description and normalized statuses. One submission is one
hardware attempt. The backend draws EDCA contention below the port (the S31
MAC's `edca::contention`); an attempt carries no backoff. The port depends on
no core. [`upper-mac`](upper-mac/README.md) owns retry ladders, Retry bits,
aggregate retry selection, the reading of a BlockAck, protection and the BSS
basic rates, management budgets, the association PHY mode and the attempt
planner. Its
`block_ack/reorder.rs` owns receive reordering and `ccmp/replay.rs` checks
receive replay; its packet-number allocator serves the backend, which
numbers protected MPDUs below the port. Espressif policy parameters live in
[`oer-espressif-ieee80211-policy`](../../chips/espressif/ieee80211-policy/README.md).

`softmac/src/extensions/espressif/esp_now/protocol.rs` owns ESP-NOW peers and
protocol state. Its `security.rs` sibling owns secrets, generations and replay
state. Both use the wire codecs, the interface identities of
`softmac/src/interface.rs` and the PHY requests of
`softmac/src/extensions/espressif/phy.rs`.

`runtime/src/configuration.rs` checks the requested Wi-Fi owner topology
against a backend's service description; `runtime/src/monitor/policy.rs`
selects the frames a monitor keeps and the channels it visits.

The [RSN crate](rsn/README.md) separates secret ownership and derivation
(`crypto`), packet views and MIC handling (`eapol`), wire formats (`frames`)
and handshake owners (`state/supplicant.rs` and `state/authenticator.rs`).

Portable AP `service` retains one peer storage owner and separates peer,
security, Block Ack, management, transmit and power-save operations into
child modules. Station association parsing and link policy live in `sta`;
chip representation stays below the port.

Module namespaces do not create independent owners. Tests live beside their
owner or at the shared protocol boundary. See the
[architecture](../../docs/architecture.md#layers) for the layering contract.

## Known defects

- **RSSI "unknown" is a value:** `wire/src/scan.rs` `ScanRecord` stores RSSI
  as `i8` with `i8::MIN` as the unknown marker. `sta/src/receive.rs`
  `observe_scan` writes it when a frame lacks a signal level.
