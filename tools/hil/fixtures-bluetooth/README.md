# Linux Bluetooth fixture

Use the canonical [Linux fixture software installation](../run/fixtures.md#linux-fixture-software-installation)
route from the repository root:

```console
cargo stand fixture install --provider linux-bluetooth --dry-run
cargo stand fixture install --provider linux-bluetooth
cargo hil fixture bluetooth-check --adapter hci0
```

This library is the fixture's host side: the privileged helper
`open-radio-bluetooth` lives in `oer-tool-hil-fixture`. The
`cargo hil fixture bluetooth-check` command and the workloads that drive the
fixture return with the BLE block; until then the library is built and tested
only.

Installation builds the helper without privileges, then uses one interactive
sudo handoff for versioned activation. The policy defaults to the dedicated
`hci0`; pass a repeated explicit `--adapter hciN` to admit other locally chosen
controllers. The old wildcard adapter grant is not retained. The helper's
finite parser remains the final boundary for peer addresses, hold duration and
termination mode. The unprivileged `capabilities` operation lets the installer
and `cargo hil doctor` reject a stale helper without opening an adapter.

The adapter must be dedicated to the test. Existing connections and a hardware
rfkill block cause an operational check to reject or restore state; installation
itself does not discover, reset or exercise the adapter and performs no RF test.
The required runtime tools are Linux Bluetooth management/HCI interfaces,
`rfkill`, ordinary interactive `sudo` and `visudo`. The installer grants no
access to Cargo, the general HIL runner, itself or a shell.

The helper locks the adapter, snapshots power and soft rfkill, unblocks it,
powers down the kernel controller and acquires the exclusive HCI user channel.
It reads the controller identity and supported commands, then actually runs
DTM v2 RX and TX on LE 1M, channel 0 (2402 MHz), with 100 ms windows. TX uses
37-byte PRBS9 packets. Both runs require successful `LE Test End` responses.
Cleanup resets the controller, releases the user channel and restores and
verifies power and rfkill. Cancellation and errors run the same cleanup;
cleanup failure makes the check fail. Forced termination or device removal
can prevent restoration and must not be interpreted as a passing check.

Each invocation creates a fresh directory under `target/hil/fixture-checks/`
containing `helper.json`, `helper.stderr` and `result.json`. Missing permissions,
rejected commands, timeouts, invalid responses and incomplete cleanup fail
closed. The command needs no ESP, flashing or stand file.

This establishes DTM command acceptance on the selected adapter. It does not
establish RF delivery: `result.json` always reports `rf_verified: false`. A
zero RX count is valid here because this check has no coordinated transmitter.
No qualification claim follows from this fixture check. Other PHYs, extended
advertising and ISO/Audio are outside this finite check.

### Explicit DTM v1 diagnostic

For a controlled comparison of command versions on the same adapter, select
the v1 profile explicitly:

```console
cargo hil fixture bluetooth-check --adapter hci0 --dtm-version v1
```

The default remains v2. Both profiles use LE 1M, channel 0, 100 ms RX/TX
windows, 37-byte PRBS9 TX packets and the same two-second command deadline.
The selected profile must advertise its RX, TX and Test End commands. A missing
command, rejection or timeout fails the check; the helper never retries or
switches versions. Reset, exclusive adapter ownership and verified restoration
are unchanged. Reinstall with `cargo stand fixture install --provider linux-bluetooth`
after updating its interface; installation adds only exact check grants for
each admitted adapter, command version and DTM profile, not arbitrary command
or timeout access.

### DTM profiles

`--profile` selects what one check runs; the runner always names it:

| Profile | Sequence |
| --- | --- |
| `receive-transmit` (default) | RX, Test End with the packet count, then TX and Test End. |
| `transmit` | TX and Test End alone, for a receiving peer. |
| `receive-silence` | RX while the peer is silent, ended by HCI Reset; the report sets `rx_ended_by_reset` instead of a count. The Intel AX211 never completes Test End after an RX test that received nothing. |

The DTM scenario preflight checks that the installed policy grants every
profile.

Check report schema 3 records `dtm_version`, `profile`, `rx_ended_by_reset` and both
advertised command profiles.
The runner requires the reported version to match the requested version.
DTM RF scenarios continue to require v2; a passing diagnostic v1
check is not substituted for their peer evidence. The helper uses a local typed
TX v1 command correction because the pinned `bt-hci 0.10.1` assigns that command
the Read Supported States opcode; its parameters and completion handling still
use `bt-hci`. Socket tests exercise the corrected TX command bytes.

## Kernel ATT connection parameters

The `att-parameters --adapter hci0` helper lease requires an initially
powered-off dedicated adapter and sets only the kernel's default LE connection
minimum/maximum interval (7.5 ms), latency (zero) and supervision timeout (2 s).
It snapshots those four values through MGMT Read Default System Configuration,
persists a root-owned recovery journal before mutation, and verifies Set Default
System Configuration by readback. It never edits BlueZ configuration, bonds,
per-peer parameter lists or another controller. The DUT must still report the
actual 7.5-ms interval; a cached peer override is a failure, not a slower fallback.

The runner holds the helper's stdin open. EOF, cancellation or the fixed 120-s
lease expiry power down the dedicated adapter and restore/verify its original
defaults and soft rfkill state. Successful cleanup removes the journal. Forced
termination or incomplete recovery leaves
`/run/open-radio-bluetooth/hci0.att-parameters.json`; subsequent fixture preflight
and privileged operations refuse to use that adapter. With no active connection,
explicitly recover the matching adapter before running another scenario:

```console
sudo /usr/local/libexec/open-radio-bluetooth restore-att-parameters --adapter hci0
```

Recovery validates the adapter and rfkill identities and never replaces the
saved snapshot with current settings. Reinstall the helper after changing its
interface. Per-run `att-parameters.stderr` records the original and selected
values; the runner requires both the ready and restored handshakes plus a
successful helper exit. These values are test configuration, not production
connection policy or qualified PHY budgets.

## ESP and adapter RF scenario

Add the selected adapter to the private stand file:

```toml
[bluetooth]
adapter = "hci0"
```

With the ESP connected at the configured serial port, run:

```console
cargo hil doctor bluetooth-dtm-bidirectional
cargo hil run bluetooth-dtm-bidirectional
```

The runner reserves the board and adapter, builds and audits the separate
`bluetooth-dtm` image, flashes it and drives framed, boot-correlated DTM
commands. This image composes the production Bluetooth controller without
the Wi-Fi runtime. It provides Reset, Receive, Transmit and Test End on LE 1M,
channel 0 with the same 37-byte PRBS9 payload as the helper. Active ESP tests
expire after 30 seconds; expiry and HCI errors attempt Reset, retain a failed
state and require a board reset before another test.
USB responses write and flush within one two-second deadline, including
responses whose encoded length is an exact USB packet multiple.

Test End and logical Reset share bounded production scheduler stop and exact
descriptor retirement. A deadline or ownership fault retains the graph and
fails the command; it never reports successful cancellation or reclaims
hardware-owned memory. The quiet controls exercise this path without packets.

The scenario first holds ESP TX while the helper counts packets on the PC.
With the PC adapter restored and inactive, each quiet cycle checks RX/Test End
and RX/Reset/RX/Test End, using 300-ms receive windows without a board reset.
The workload's `quiet_cycles` accepts 1 through 1000; the catalog requests 100
per boot. Omitting it preserves three End controls and one Reset restart.
Every Test End count must be zero and every
Reset restart must finish. Partial counts are retained on failure.
Finally ESP remains in RX throughout another helper invocation: the helper's
RX window measures PC silence, and its TX window supplies packets to ESP.
Thus each receiver has both a positive-delivery requirement and a zero-count
control with the other transmitter inactive. This is not an idle-state RF
emissions test or a packet-error-rate measurement.

Both positive counts must reach the scenario's `minimum_packets`, and both
silence counts must equal zero.
DTM events retain cumulative production RX and sequence-deadline diagnostics
in `protocol.jsonl`. These observations do not acknowledge hardware or change
packet accounting. Command acceptance alone cannot pass. Every
completed boot writes `bluetooth-dtm.json`, peer helper reports and the framed
UART transcript into the ordinary immutable HIL bundle, including on failure.
ESP Reset and adapter restoration are checked independently of RF acceptance;
the peer identity must match across directions. The catalog scenario requests
two boots and stops at the first failure. It establishes
only the tested LE 1M DTM link; general BLE readiness remains with qualification.

The connection helper report uses schema 14. The installer and runner require
the same advertised helper contract; reinstall the Linux Bluetooth fixture when
that contract changes. Advertising Encryption in this exchange does not qualify
encrypted traffic: key exchange, MIC/counter handling and encrypted interoperability
require their own evidence.

## Kernel ATT fixture

The GATT and advertising workloads use a fixed ATT socket through the Linux
kernel and BlueZ, implemented in
[`att.rs`](src/att.rs). It uses the
same runner adapter lease but does not enter the helper's exclusive HCI channel.
The configured adapter must initially be powered off. BlueZ, `busctl` and user
access to `/dev/rfkill` are required; the socket binds the selected adapter's
public address. Missing access fails setup rather than changing permissions.

The owner snapshots adapter identity and soft rfkill, powers it for the test,
and verifies power/rfkill restoration on completion or error. The brief BlueZ
re-registration interval after clearing rfkill has a bounded setup retry.
Cleanup results are saved with the ordinary HIL run. No helper reinstall
is required for this path.

### Connections the DUT never received

A CONNECT_IND can collide in the air with another device's response to the
same `ADV_IND`, for example a nearby active scanner's `SCAN_REQ`. The adapter
then reports the connection created and closes it as "Connection Failed to be
Established" (0x3E), while the DUT never saw it. Like any central, the
GATT and secure GATT workloads retry such a connection, up to three attempts
in all. They retry only when the DUT's own observation proves that the
connection never reached it: no new connection or comparison, not connected,
still advertising. Every other failure ends the repetition. The host's
connect alone proves nothing, because the kernel may report the socket
connected before the first procedure. An attempt therefore succeeds only once
the DUT reports that connection within two seconds; otherwise the host closes
its side and the attempt is judged as above (#385).

Every attempt lands in the observation's `connection_attempts`, with its phase,
the DUT connection number it was for, its attempt number, outcome and the
host-side error, so the loss stays visible (#357). The outcome is
`established`, `not-established` (retried), `failed` (the DUT saw the
connection) or `unobserved` (the DUT's own observation failed). A failed
observation reports its own cause together with the attempt's host-side
error, and exhausted attempts report the last host-side error. Each repetition also publishes the
measurements `connection-attempts` and `connections-not-established` (lower is
better), which `cargo hil runs history` and `runs compare` follow. The
plaintext GATT workload also publishes `connections-lost-before-att` (lower is
better): a confirmed connection the DUT reports closed by supervision timeout
(0x08) before any ATT exchange on it, listed as `lost_before_att` in the
observation (#364). The repetition still fails; on the trace image the failed
cycle's trace lands in the post-mortem. The bound of
three attempts guards against the environment; it is no acceptance threshold.
Whether a loss rate is acceptable is qualification's decision: the
`secure-peripheral-gatt` requirement bounds `connections-not-established` over
`connection-attempts` across the current runs of `bluetooth-trouble-secure-gatt`
(measurement bounds, #363).

## Encryption and key-failure modes

No catalog scenario uses the `connect-reset` encryption modes or the
`security-failure` operation at present. `connect-reset --encrypted` starts
AES-CCM with the public test LTK/Rand/EDIV from `oer-hil-protocol`, requires a
successful Command Status followed by Encryption Change for the exact handle,
and only then sends application data. `--key-refresh` issues LE Enable
Encryption again after both updates with the distinct public
`BLUETOOTH_REFRESH_*` identity and requires Encryption Key Refresh Complete on
the original handle before the second echo. The launcher accepts no arbitrary
keys or HCI commands.

`security-failure --adapter hci0 --peer <address> --failure
missing-key|wrong-key|missing-refresh-key|active-data-mic` runs one connection
on which the peer expects the named failure from the target, with optional
`--read-version-before-disconnect` after a missing-key rejection. Its reports
use schema 4 and retain at most 64 incoming HCI packets, each bounded to 258
bytes, with monotonic times relative to submission of LE Enable Encryption;
the probe has an eight-second deadline. The runner's security-failure preflight checks password-free
admission of every failure mode with an invalid peer address, which the CLI parser rejects
before any adapter acquisition.

## Secure Trouble GATT fixture

Run `cargo hil run bluetooth-trouble-secure-gatt` from the repository root;
no interactive terminal or stdin response is required, including over SSH.
It uses the dedicated, initially powered-off adapter and ordinary kernel/BlueZ
path, not the helper's exclusive HCI channel. No helper reinstall is needed for
this path. BlueZ D-Bus access, `busctl`, `/dev/rfkill` access and a DUT without an
existing BlueZ device record are required. A preexisting record is never deleted
or adopted, even if it is unpaired or left after an interrupted earlier run.

The fixture registers its own `DisplayYesNo` agent on the same bus connection
that issues `Pair`; it does not replace the default desktop agent. It accepts
callbacks only from that BlueZ owner for the selected DUT and rejects other
pairing methods. BlueZ restart invalidates ownership. Read/write/notification
verification uses BlueZ GATT; notifications are received through `AcquireNotify`,
not inferred from a cached characteristic value.

The scenario first verifies that plaintext value reads/writes and CCCD writes
receive security errors. It then explicitly declines one Numeric Comparison
attempt. The positive attempt compares the independent numbers received from
the DUT's framed USB console and the selected BlueZ agent callback. Only a match
produces explicit positive replies to both peers; the DUT reply is bound to the
exact boot and pending request. Missing, invalid, stale or different challenges
cannot be accepted. This is an automated HIL test operator, not proof of human
presence or a production auto-pairing policy. The standalone application's
manual console remains unchanged. The agent callback has a 25-second limit and
D-Bus calls a 35-second limit.

After authenticated enrollment it checks value reads/writes and independently
received notifications, disconnects, and repeats using the retained bond without
a new comparison. It then requests full physical Controller cold shutdown and
restart while retaining the application's RAM store. The SoC boot must remain
unchanged, the old HCI handle must be closed, and a fresh Host must resume
authenticated encryption without pairing. The characteristic resets to zero;
the bond does not. The DUT has one RAM-only bond slot. BlueZ may temporarily
persist the peer bond on Linux; cleanup disconnects, removes only the record
whose absence was checked before this experiment, verifies removal, unregisters
the agent, and restores power/rfkill. Any cleanup error prevents PASS. Forced
process death can leave a record: the next run refuses it rather than silently
deleting unknown state. Review that exact record before manually removing it.

`trouble-secure-gatt.json` retains value-only snapshots, independent peer
observations, comparison identities, the automated confirmation mode and cleanup
results. No LTK/IRK is exported.
The source scenario is not hardware qualification. It does not prove power-loss
bond persistence, storage-fault recovery, RF quality or terminal-fault/cancellation
disposition during retirement.
