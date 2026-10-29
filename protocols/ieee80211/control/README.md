# Wi-Fi HIL lifecycle and traffic

`oer-ieee80211-control` is the host-safe, `no_std` HIL wifi vocabulary. It depends on the generic HIL message/frame contract; the framework does not depend on it. Its `registry` feature exposes descriptors and `messages.lock` for host composition. Image services advertise their mounted keys through the agent.

## Station wire module

This dictionary defines `wifi/station/start`, answered by `base::Accepted`
for admission, and `wifi/station/link` for `Associated`, `Authorized` and typed
`Disconnected` states. A link event carries a station-attempt epoch within its
boot. `Authorized` means installed RSN keys and an open controlled port.
`Authorized` retains the runtime BSSID and channel. `StationPowerSave` admits only `Off`.

`ConnectStation`, `DisconnectStation` and `ScanStation` dispatch directly to
station Control and return typed `StationAdmission` values. Their callback
refusals and terminals publish `StationCommandFinished` with the original
request identity and complete runtime cause. `GetStationStatus` mirrors the
product status. Each command kind retains one pending HIL identity; a duplicate
is refused as `InFlight`, without replacing the first requester. Policy commands
and explicit HIL commands use disjoint correlation spaces. Unsolicited policy
terminals and unmatched correlations are explicit event origins. A stopped
Control completes retained unexecuted requests once as `NotExecuted(Stopped)`;
its product status remains readable before installation and after stop through
a read-only runtime projection, without a separate HIL status cache.

The runtime owns its product vocabulary and converts core values internally.
This binding mirrors it with exhaustive `From` implementations and has no core,
chip, HAL or clock dependency. `StartStation` still supplies image configuration;
`StopStation` disconnects and cooperatively stops the service. Diagnostic
`StationGroupKey`, counter, receive-drop and ARP records come from the image's
trace collectors. They are not product status fields. TX/MSDU refusals retain
their typed product causes. Startup unavailability is `StationServiceEnded`,
without inventing a Control disconnect reason.

Traffic originates at the host/peer. The device uses normal DHCP and ICMP echo
reply services; no probe command, target ping generator or echo window is mounted.
The Wi-Fi station family checks peer ping counts against independent request/reply captures,
including packet identities and one-second pacing, and keeps the GTK/ARP, air,
reconnect, departure and hardware/model key evidence gates.

`NetworkCredentials` owns an SSID of 1..=32 bytes and a passphrase of 8..=63
bytes. Construction and binary decoding both enforce these bounds. It has no
`Clone` or `Copy`; drop erases its storage. `clear_passphrase` erases the entire
passphrase buffer, preserves the SSID, and prevents reusing the cleared owner
in another wire command. Debug and human-readable serialization replace the
credentials with `<redacted>`; human-readable decoding is rejected. The host
records command identity and time, never command payloads. Transport Debug
prints metadata and payload length only. A target must not echo credential
wire bytes into UART evidence.

With `oer_trace`, `collector::Observations` translates the existing dictionary
through an image-owned generic HIL collector and injected timestamps. HIL
summaries use independent aggregate counts, plus correlation boundaries for
GTK and final deauthentication. Bounded history exhaustion remains explicit.
`StationStopped` carries the frozen capture description. The generic capture
service serves its immutable records and counters. Service failures carry the
exact serialized facade value and its schema key instead of a second, lossy
failure dictionary; schema mismatches and malformed encodings are refused.
