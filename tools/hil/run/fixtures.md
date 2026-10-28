# HIL host fixtures

Network scenarios prepare and restore a controlled access point, clients and
passive observers; Bluetooth and network helpers are installed once per host.
This guide covers their preparation, evidence boundaries and installation; the
[host guide](HOST.md) covers the stand file that names them.

No HIL scenario uses the network fixtures (`oer-hil-family-ieee80211-fixture`);
the network sections below describe what they do and what a Wi-Fi scenario
relies on.

## Prepare and restore network fixtures

The controlled OpenWrt AP and HIL host share the fixture LAN. Reverse flows
use the local IPv4 route selected for the discovered target. External AP
fixtures provide compatibility workloads; exact-delivery scenarios require
the controlled fixture declared by the scenario.

Every network scenario owns a prepared station AP, including target-AP tests
that first qualify a station connection. OpenWrt `radio` and `ap_section` identify
the UCI resources; the transient netdev is not used to infer ownership. The
runner discovers PHY/AP capabilities, applies the scenario's HT20/HT40/HE20
profile, channel, WPA2 credentials and WMM, then checks enabled hostapd, generated
HT/HE settings, width and center frequency. `phys` is an access policy, not
hardware discovery. Original options, pending UCI edits and radio up/down state
are restored; HIL does not commit temporary settings to flash.

AP scenarios derive target bandwidth from their link profile. A router hosting
their managed client uses the same primary and secondary channel. These scenarios
start a fresh epoch of the selected radio; other radios and the wired uplink are
not brought down. Scoped client forwarding/VIF cleanup remains a separate owner.
Every AP workload records its primary client's link counters over each
session (`primary_client_link`, tagged by client), so a lost datagram names its
side. The OpenWrt client reads its station counters and TID 0's AQM drops over
SSH. The laptop client reads `iw dev wlan0 station dump` (`tx_failed`,
`tx_retries`), `wlan0`'s `tx_dropped`, and, through the helper's `client-aqm`
operation (schema 16), mac80211's TID 0 AQM drops and overlimits: its root
queueing discipline is `noqueue`, so a packet it drops before it becomes a
frame shows there and leaves no gap in the AP's sequence.
`fixture-applied.json` records actual settings without network credentials.
Cleanup failures are retained and quarantine subsequent network workloads in
the same runner invocation.

Fixture preparation can be exercised without opening the serial port, building
firmware, resetting or transmitting traffic from the DUT:

```console
cargo hil fixture check <scenario>
```

This command uses the same prerequisites and profile owner as `run`, opens and
stops the required OpenWrt packet captures, restores the AP and writes its report
to `target/hil/fixture-checks`. Control scenarios also exercise AP stop/restart.
Scenarios requesting the independent laptop observer exercise Linux monitor
setup, capture readiness, tshark decoding and managed-interface restoration;
`fixture-monitor.json` retains the capture result. This passive check uses a
synthetic parser filter and sends no target traffic.
It does not establish target associations or qualify target throughput.

A station UDP workload may declare `induced_protection`. The laptop radio
then acts as a real, standard-conformant peer of the OpenWrt station fixture:
`non-ht-member` joins its BSS with HT, VHT and HE disabled, so the AP must
advertise non-HT mixed HT protection; `overlapping-legacy-bss` hosts an
802.11b BSS (1, 2, 5.5 and 11 Mb/s, no ERP or HT element) on the AP's channel,
so the AP must set ERP Use_Protection. No AP setting is forced. Before the
workload, the independent air observer must see every beacon of a two-second
window carry the expected protection within ten seconds; otherwise the run is
blocked with `protection-peer/fixture-protection.json`. The legacy BSS needs
the laptop's regulatory domain in the optional `[legacy_bss] country` lab
section and the schema-13 network helper.

Every 802.11 air observer (the laptop monitor, the OpenWrt TX-monitor tap and
the probe-load management capture) decodes its capture through
`oer_hil_family_ieee80211::evidence::air`: one tshark run with a fixed field set yields typed
`AirFrame` records, and each analyzer works on those frames in memory. A
malformed record, unparsable field or invalid timestamp fails the whole
capture; absent fields stay absent.
`doctor` checks available tools and capabilities without applying a profile;
a successful doctor result does not assert that current radio settings already
match the selected scenario.

## Collection and failure boundaries

Capture handles acknowledge readiness before the session starts. Dumpcap's
opened-file notification and tcpdump's opened-interface notification replace
startup sleeps. The runner explicitly stops capture after session collection;
traffic duration does not set an early capture stop. Independent process
watchdogs and file limits remain failure bounds. Passive observers use the AP's
actual primary frequency, width and center frequency, including HE20 geometry.
Tshark parsing and monitor setup/teardown are invoked by the runner.

Host UDP collectors finish from the correlated `network::Finished` transport count.
Complete delivery returns immediately. If packets remain undelivered, a two-second
delivery deadline bounds collection after that event; reaching it records
`delivery-deadline`, never proof of a drained radio. Each `*-reception.json`
retains the expected/unique/undelivered packet counts, partial bursts and the
termination reason even on target failure, I/O error, cancellation or unwinding.
There is no additional fixed reception window after the configured workload.

Serial I/O waits for descriptor readiness or explicit command/shutdown events.
SIGINT/SIGTERM notifications wake both serial protocol waiters and UDP collectors;
periodic polling is unnecessary for cancellation. Physical USB reset timing and
exclusive-port acquisition remain owned by the serial setup boundary.

For multi-client RX offers, `minimum_host_offer_percent` independently checks
bytes accepted by host UDP send calls over both the requested window and the
sender's elapsed time. The AP comparison scenarios require 95%. An under-offer
invalidates the requested load condition; it does not identify a DUT delivery
fault. No criterion means `not-assessed`, never an implicit load validation.
Host socket admission is not an on-air transmission measurement.

Diagnostic image features can change scheduling and linked code placement.
Compare performance only with the recorded image/configuration identity; a
more instrumented image is a separate experiment, not interchangeable evidence.

## Linux fixture software installation

Linux network and Bluetooth fixtures share one provisioning workflow. Preview
the exact provider plan first; these commands do not execute any listed build,
capability, sudo or hardware step and do not load the stand file:

```console
cargo stand fixture install --provider linux-net --dry-run
cargo stand fixture install --provider linux-bluetooth --dry-run
```

The preview includes `existing_installation`: `fresh` or the selected versioned
generation. It reads that generation's receipt and verifies its artifact set;
new roles require absent stable paths. A missing installed artifact, an unmanaged
path or an unfinished transaction fails the preview without changing host state.
Sudoers policy checks are listed in `pending_privileged_checks`: its directory
need not be searchable by the operator. The installer repeats the artifact
classification under its locks and checks the existing policy before applying.

The helper `capabilities` and sudoers validation subprocesses each have a
five-second execution deadline; timeout is an installation/preparation failure.
Bluetooth preparation and execution share the versioned contract in
[`bluetooth/model.rs`](../../lab/fixture/src/bluetooth/model.rs).
Synchronous HCI command receive failures report the command identity, elapsed
time, packet count and last event identity in the helper's existing error
report. They do not record event payloads or keys, retry commands, or extend
the two-second deadline when unrelated events arrive. A DTM Test End timeout
is a peer-check failure, not evidence that the DUT has stopped transmitting.

Like every Cargo alias, `cargo hil` may compile the runner before it can print
the plan. That Cargo startup is not an installer plan step. Missing helper
binaries appear as `missing`, while present files remain `present-unverified`.

Install exactly one provider with interactive authorization:

```console
cargo stand fixture install --provider linux-net
cargo stand fixture install --provider linux-bluetooth
```

Installation is stand maintenance: it claims only
`fixture-software:<provider>` exclusively and goes ahead of every waiting
request, whatever the balances. Every lease holding a conflicting claim,
such as a run of that provider, is preempted at once with the reason
`fixture maintenance: <provider> install by <owner>`: `SIGTERM` with the
ordinary cleanup, `SIGKILL` after the grace, no further charge, a history
record and a notice to the user. Waiting requests it overtakes say so in
their wait line, and a preempted run ends interrupted, not failed; rerun
it. Later runs of the provider wait for the installation. Runs take the
software lease only once granted, so a queued run never blocks an
installation. Preparation happens before queueing; `sudo` prompts only once
the claim is granted.

Bluetooth policy admits only named adapters. It defaults to the dedicated
`hci0`; repeat `--adapter hciN` to install a sorted finite allow-list. Reinstall
with the adapters selected by the private stand file before using
another controller.

Cargo is the single installation entry point and runs as the unprivileged
operator. Linux is required; installation is not supported on other hosts.

Preparation runs as the operator. Network preparation retains the pinned
hostapd build, patch and provenance, the pinned single-applet udhcpc build
and provenance, and the locked station/probe helpers and fixed launchers.
The [schema-16 station contract](../linux-net/README.md#station-contract)
supports open or WPA2 authentication, static/absent IPv4 or a maintained
DHCP lease, and a bounded bidirectional echo workload without NetworkManager.
Bluetooth preparation builds only its finite helper, fixed launcher and
installer with the same locked root `target/debug` output; it does not
build or download hostapd. The runner rejects an invocation already running as
root before starting either build. It then transfers the foreground terminal
to ordinary `sudo`; no password pipe or askpass path exists, and the general
HIL runner never runs as root.

The privileged apply owner imports the content-addressed bundle into
`/var/lib/open-radio/fixture/<provider>/generations/`. It validates artifact
bytes, root ownership/modes, the exact helper contract and a candidate policy
against the effective sudoers policy before publication. Stable commands under
`/usr/local` enter finite provider launchers. For an operational command the
launcher first acquires the provider software lease, rejects unfinished or
unprovable installation state, resolves one committed `current` generation and
then executes that generation's root-owned helper. The lease remains live for
the helper call. Switching `current` is the commit point. The network helper
uses hostapd from the selected generation, so one invocation cannot combine
helper and daemon bytes from different generations. The read-only
`capabilities` operation remains available to installation verification without
operational admission or hardware effects.

One persisted transaction journal covers stable links, policy and generation
selection. Pre-commit failure restores the prior files and sudoers policy.
Post-commit software verification failure rolls back; a receipt-write failure
keeps the activated generation and leaves an explicit recovery-required journal.
The next authorized install recovers that journal before considering a new
bundle. The prior managed generation is retained. Unmanaged or incomplete
installations require explicit operator recovery; installation does not import
arbitrary existing files. Reinstalling the same bundle verifies it and refreshes its fixed-path
receipt without switching generations. Generations are not automatically
deleted while their lifetime is unknown.

Every runner operation that may use a provider holds its shared software lease
for the complete fixture/run boundary. The empty root-owned lease file is
`/var/lib/open-radio/fixture/<provider>/session.lock`; it is persistent,
read-only to the operator and is never replaced by repeat installation or
upgrade. Kernel `flock` ownership, not file contents, identifies a live owner,
so clearing `/run` or rebooting does not require reinstallation. All applies
serialize through one root-owned installation lock, then installation requires
the selected provider's exclusive lease and fails while such a session is
active. Installation never
stops hostapd, an adapter or another service to force an upgrade. A directly
started network AP is also detected through its root-owned pid file. The lease
is software-update ownership only and never acquires the DUT, opens SSH or
discovers fixture hardware.

Operational admission checks the existing transaction journal, `current`
selector, successful receipt and selected artifact identity after taking the
same shared lease that conflicts with apply. A pending/corrupt journal, missing
or unsafe selector, invalid receipt, writable state path or artifact mismatch
returns `recovery-required` before a hardware-touching helper runs. Only the
installer performs recovery. Its direct generation `capabilities` check does
not recursively acquire the operational lease. Launcher descriptors are closed
by long-lived network daemons; direct AP lifetime remains protected by the
existing pid-file check, while foreground helper and runner leases retain their
original lifetimes.

After activation the installer checks installed bytes, policy and only the
helper's non-hardware `capabilities` operation. It does not run `doctor`,
`fixture check`, Bluetooth DTM/connect-reset, identity discovery, serial, HCI,
SSH, network mutation, flashing or RF checks. The fixed root-owned receipt at
`/var/lib/open-radio/fixture/<provider>/receipt.json` reports the generation,
source dirty/content identity, artifact and policy hashes, previous generation,
checks and terminal/recovery state. It contains no lab credentials and is not
HIL evidence. A successful receipt means software is
installed consistently; privileged platform behavior and hardware acceptance
remain separate operator checks.

| Operation | Software/filesystem writes or privilege | Host network mutation | Adapter reset/rfkill | RF transmission | DUT required |
| --- | --- | --- | --- | --- | --- |
| `fixture install … --dry-run` | No installer step; Cargo may build the runner | No | No | No | No |
| `fixture install --provider …` | Builds as user, then interactive privileged system writes | No | No | No | No |
| installed helper `capabilities` | Read-only software inspection; network helper uses its existing finite sudo grant | No | No | No | No |
| `fixture bluetooth-check` | Writes a local report and uses the finite helper grant | No | Yes, with restoration | Yes, bounded DTM TX | No |
| `fixture check <scenario>` | Writes a local report and may use finite helper grants | Depends on scenario | Depends on scenario | Depends on scenario | No |
| `run` / `run-all` | Builds and writes evidence; uses selected fixture grants | Depends on scenario | Depends on scenario | Yes for radio scenarios | Yes |

`cargo hil doctor` verifies installed helper schemas and non-interactive finite
sudo availability before a selected scenario takes ownership. It remains a
separate readiness inspection, not post-install verification. Local Linux AP
profiles are generated from station credentials and
`station_fixture.country`, `channel` and CIDR `address`; HT/HE mode comes from
the scenario. The DHCP range excludes the AP address and stays inside its
subnet. Static station addresses must agree with that subnet and gateway. The
helper receives these values on stdin and keeps the temporary hostapd
configuration under root-owned `/run` with private permissions; stop/cleanup
removes it. No installed credential profiles are consumed. The dedicated
network provider continues to own only `wlan0`.

The runner subscribes to hostapd control events, confirms ENABLED and actual
HT/HE mode, WPA2, channel geometry and IPv4 address before workload execution.
It records these non-secret settings in `fixture-applied.json`. The Linux helper
keeps hostapd startup diagnostics in a group-readable runtime log and includes
its bounded, credential-redacted tail in preparation errors before cleanup.
Debug logging ends before the workload starts; cleanup removes the runtime log.
The Linux helper owns only `wlan0`. While it does, a per-boot
`/run/NetworkManager/conf.d/90-open-radio-hil.conf` keeps NetworkManager from
managing any `wlan0` netdev, including each one the helper recreates;
cleanup removes it and returns `wlan0` to managed mode.

AP scenarios wait for the matching `wifi::AccessPointStarted` event and validate
the successful `Idle` to `AccessPoint` transition before starting either external
client. Sending the start command is not readiness: the target completes the
request after activating AP RX interrupts, publishing the first beacon and
applying its network configuration. Initialization failures produce a start
failure instead of an early success followed by a stop error.
The controlled Linux client starts with its network disabled. The runner attaches
through the group-accessible private supplicant control socket before enabling
that network, then waits for events and `wpa_state=COMPLETED`. It does not poll
status on a timer. The connection watchdog is 20 seconds. In each cycle's
`linux-client/` directory, `helper.log` records setup failures, `control.jsonl`
records timestamped events, status replies and scan results, and `connection.json`
records the final state, last rejection/disconnect and outcome. Unknown states
remain unknown rather than being classified as discovery failures. The transcript
is bounded to 4096 records and records no credential-setting commands. Connection
artifacts survive restoration of the managed interface. When the OpenWrt fixture
has `monitor_interface` configured, AP scenarios capture management and control
frames before enabling the Linux client and retain capture through traffic and
AP stop. A failed connection also finishes the capture before restoring clients.
The cycle owns `management.pcap` and capture counts in `management.json`. Capture
readiness and stop are explicit events; immediate packet delivery preserves short
connection captures. Empty captures and capture-socket drops report incomplete
fixture evidence. The monitor is removed before client restoration and on errors.
This on-router monitor is not an independent receiver: missing ACKs in its tap
alone do not prove that no ACK was transmitted over the air.
Diagnostic firmware retains the first failed probe response receiver, Sequence
Control, publication/completion times, final rate and retry report in UART output.
The record identifies a terminal failure; ordinary retry attempts do not create it.
Probe responses use one hardware attempt and a global 10-ms admission interval.
Excess requests are discarded without deferred response timers; changing sender
MAC does not bypass the budget. Authentication, association, EAPOL and data retain
their own retry policy. `tx_probe_ack_timeouts` is a subset of
`tx_hardware_failures`, not a successful delivery count. The AP gate reconciles
this named subset and unacknowledged disconnects against the total; unrelated
failures, timeouts, collision limits and saturated totals still fail.
This bounds software retry amplification, not RF contention or interference.
Unlike [hostapd's no-ACK submission](https://chromium.googlesource.com/chromiumos/third_party/hostap/+//src/ap/beacon.c)
for wildcard broadcast probes, this backend
still waits for the ordinary hardware completion and records a missing ACK.
Control transcripts include host Unix timestamps for comparison with pcap; clock
offset between hosts must be checked before interpreting sub-millisecond timing.

For multi-flow UDP TX with driver observation, terminal `OTXFLOW` records describe
flow 1 at socket admission and at the radio's claim of an Ethernet owner. Once
that flow shows activity, both boundaries are printed, including a boundary
with zero packets. `first_us` measures time from the diagnostic interval start
to first admission; `idle_us` includes silence before the first packet and after
the last. `gap_us` measures only intervals between admitted packets. `errors`
counts terminal socket failures, which close `pending_us` without counting a
packet. These are supplemental UART diagnostics, not MAC completion or host
reception evidence. The current per-flow records do not correlate individual
packets with hardware publication and completion.

`cargo stand fixture install --provider linux-net` first builds hostapd as
`cargo stand fixture build-hostapd` does, without root, then installs the resulting binary, build provenance and helper through
interactive sudo. The build uses the pinned hostapd release and reviewed
[coexistence patch](../linux-net/hostapd/README.md). It requires a C compiler,
make, pkg-config, libnl3 and OpenSSL development files, curl, tar and patch.
Verified cached outputs can be reused without downloading or compiling again.
Updating the helper contract requires rerunning the installer.

For `local-linux`, `station_fixture.coexistence` selects `respect` (default) or
`force-ht40`. The latter applies `noscan=1` only to HT40 scenarios; the OpenWrt
patch also skips client coexistence/intolerance handling in this mode. HT20 and
HE20 retain normal policy. The selected policy is recorded in fixture evidence.
Either policy still fails preparation when actual channel geometry differs
from the scenario: requested 40 MHz never silently becomes an accepted 20 MHz run.

