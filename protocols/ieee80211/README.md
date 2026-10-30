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
| `upper-mac/src/block_ack/recipient.rs` | One receive agreement's ADDBA negotiation, inactivity and reorder gap |
| `upper-mac/src/block_ack/banks.rs` | An association's receive agreements: ADDBA, BlockAckReq, DELBA, retained frames and owed replies |
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
| `wire/src/sae.rs` | SAE Commit and Confirm body codecs (byte values of scalar, element and confirm) and SAE extension element IDs |
| `wire/src/status.rs` | Status and reason codes |
| `wire/src/ethertype.rs` | The EtherType values the 802.11 side names (EAPOL, WAPI) |
| `wire/src/tsf.rs` | The Timestamp field and the time unit |
| `sta/src/security.rs` | Station security policy: candidate admission and RSN selection |
| `ap/src/security_policy.rs` | Access point security policy and its advertised elements |
| `wire/src/data.rs` | MSDU fields (`MsduHeader`, `MsduParts`), data encapsulation plans, MSDU and A-MSDU decapsulation |
| `wire/src/station/data.rs` | Data codecs, with A-MSDU framing in `station/data/amsdu.rs` |
| `wire/src/sequence.rs` | Twelve-bit sequence numbers: the checked Sequence Number subfield |
| `upper-mac/src/sequence.rs` | Modulo-4096 sequence arithmetic (`SequenceSpace`) and the separate management/non-QoS and per-TID TX sequence owners of the station, the access point and ESP-NOW |
| `upper-mac/src/ccmp.rs` | CCMP transmit packet numbers and the receive replay frontier of every CCMP user |
| `contracts/security` | The CCMP-128 temporal-key owner that RSN, the port, the software cipher and a backend share |
| `rsn/src/pmksa.rs` | The PMKSA cache of the station and of the access point |
| `rsn/src/authenticator.rs` | The access point's four-way handshake, with its SAE responder in `authenticator/sae.rs` |
| `upper-mac/src/duplicate.rs` | Peer-scoped receive retry history |
| `wire/src/qos.rs` | User priorities and access categories |
| `upper-mac/src/qos.rs` | Admission-control downgrade of an access category |
| `upper-mac/src/vif.rs` | Receive routing for one same-channel STA plus SoftAP pair |
| `upper-mac/src/monitor.rs` | The frames a monitor keeps and the channels it visits |
| `arbiter/src/interface.rs` | Virtual-interface identities and their channel-context bindings |
| `arbiter/src/configuration.rs` | The requested owner topology against a backend's service description |
| `esp-now/src/configuration.rs` | Exclusive station-address topology for standalone ESP-NOW |
| `sta/src/port.rs` | The station's tokens on a lower-MAC port and its numbered pairwise key |
| `wire/src/channel.rs` | Validated 2.4/5 GHz channels and 20/40 MHz geometry |
| `port/src/regulated.rs` | The regulatory view of a channel: the frequencies it occupies (`RegulatedChannel`) |
| `wire/src/channel_switch.rs` | Channel Switch Announcement parsing and encoding |
| `wire/src/phy.rs` | Validated non-HT, HT and HE SU rates |
| `wire/src/rates.rs` | Legacy rates of the (Extended) Supported Rates elements |
| `wire/src/frame.rs` | Frame Control with named subtypes, the data/management MAC header codec, Sequence Control access and the QoS Control field |
| `wire/src/management.rs` | The management MAC header, Capability Information and probe requests |
| `wire/src/beacon.rs` | Beacon encoding, DTIM/TIM stamping and template element edits |
| `wire/src/monitor_injection.rs` | Header geometry of a monitor-injected MPDU |
| `wire/src/extensions/wmm.rs` | WMM AC parameters and vendor IE parsing |
| `wire/src/extensions/espressif/esp_now.rs` | ESP-NOW framing and protected-envelope validation |
| `esp-now/src/extensions/espressif/esp_now/reassembly.rs` | Caller-owned ESP-NOW v2 datagram storage |
| `sta/src/events.rs` | The station core's events: lost tokens and data, dropped RSN input, scan table outcomes |
| `upper-mac/src/block_ack/banks.rs` (`RxBlockAckEvent`) | The receive agreements' events: containment, reorder decisions, agreement start and stop |
| `esp-now/src/extensions/espressif/esp_now/security.rs` (`EspNowPeersCleared`) | The keys shutdown cleared, the one fact its result does not return |
| `runtime/src/station/events.rs` | The station service's events and `StationEvents`, the sink a composition lends the service |

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

[`arbiter`](arbiter/README.md) owns one radio's VIF identities and
capability-checked station, AP and monitor topology, and decides the channel
of a port shared by a station and an access point. The upstream BSS chooses it; the AP follows
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
`block_ack/banks.rs` owns an association's receive agreements and reordering.
Its `ccmp.rs` is the one owner of CCMP numbering: the receive replay frontier
(`CcmpReplayFrontier`; `ccmp/replay.rs` checks an association's keys) of the
station, the access point's protected management frames, the RSN management
receiver and ESP-NOW peers, and the transmit packet-number allocator, which
numbers each fresh protected MPDU before the port takes it (the station's
pairwise key in `sta/src/port.rs` holds it); the backend encrypts with the
supplied number. Its `sequence.rs` owns the transmit sequence spaces of every
role. Espressif policy parameters live in
[`oer-espressif-ieee80211-policy`](../../chips/espressif/ieee80211-policy/README.md).

`esp-now/src/extensions/espressif/esp_now/protocol.rs` owns ESP-NOW peers and
protocol state. Its `security.rs` sibling owns secrets and generations; each
peer's packet numbers and replay frontier are `upper-mac`'s CCMP owners. Both use the wire codecs, the interface identities of
`arbiter/src/interface.rs` and the PHY requests of
`esp-now/src/extensions/espressif/phy.rs`. ESP-NOW's `configuration` narrows
the arbiter's checked `WifiPlan` to exclusive standalone ESP-NOW ownership.

The cores and the runtime keep no counters or diagnostics of their own work.
An owner (the station, its data machine, the receive agreements, the
ESP-NOW peer table) takes `&mut impl oer_events::EventSink<E>` as a call
parameter and reports what it dropped, refused or decided as a value of its
own event type `E`; it never stores the sink. The station service holds the
sink its composition chose (`O: StationEvents`, by default `()`) and lends it
to each call of its cores. One fact is reported in one place: a refusal or
result a call returns is its caller's to count, and an owner reports only what
it decides and absorbs. What an event becomes, the time it is bound to and
how it leaves the device belong to the facade or image; `()` keeps nothing and
builds no event. No IEEE 802.11 package depends on `oer-trace`, transport
serialization or HIL. Counts that decide protocol behaviour (packet numbers,
replay frontiers, retry and probe budgets, beacon deadlines) stay in their
owners.

The [`runtime`](runtime/README.md) is the asynchronous glue: it waits on
port events, deadlines and commands, hands each input to the cores and
carries out their actions through the port. It decides no protocol question
and neither parses nor builds frames; it depends on neither `wire` nor
`cipher`, and runs the station's RSN key-data unwrap in the engine the image
supplies through the `oer-crypto` contract. The cores never wait. `cargo test -p oer-test-layers` checks both,
and that `cipher`, `port` and `model` take only the layers below them.

The [RSN crate](rsn/README.md) separates secret ownership and derivation
(`crypto`), packet views and MIC handling (`eapol`), wire formats (`frames`),
handshake transitions (`state/supplicant.rs` and `state/authenticator.rs`)
and their orchestration on both sides (`supplicant` and `authenticator`),
and the PMKSA cache of both roles (`pmksa`).

Portable AP `service` retains one peer storage owner and separates peer,
security, Block Ack, management, transmit and power-save operations into
child modules. Station association parsing and link policy live in `sta`;
chip representation stays below the port.

Module namespaces do not create independent owners. Tests live beside their
owner or at the shared protocol boundary. See the
[architecture](../../docs/architecture.md#layers) for the layering contract.

`ieee80211/wire` depends on `oer-wire` alone; it holds no state, profile or
regulatory decision. The station keeps a scan observation's signal level as
`Option<i8>` (`sta::scan::BssObservation`), and chooses the RSN capabilities
of its association (`sta::security::personal_rsn`) that the codec encodes
(`station::SelectedRsn::rsn`).
