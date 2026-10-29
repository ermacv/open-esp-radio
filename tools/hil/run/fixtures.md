# HIL host fixtures

Network scenarios prepare and restore a controlled access point, clients and
passive observers; Bluetooth and network helpers are installed once per host.
This guide covers their preparation, evidence boundaries and installation; the
[host guide](HOST.md) covers the stand file that names them.

The current runner's fixture-provider registry is empty
(`tools/hil/run/src/scenario.rs`). Wi-Fi and lower-MAC workloads perform
managed-router operations in their own family crates. `cargo hil fixture
check SCENARIO` checks prerequisites and calls registered providers; an empty
provider list does not apply or restore an AP profile.

The finite helper interfaces and their operating effects are documented in
[Linux network fixtures](../linux-net/README.md) and
[Linux Bluetooth fixtures](../linux-bluetooth/README.md). The stand file and
physical fixture leases belong to `tools/lab/model` and `tools/hil/session`;
provider software leases are separate from those device and fixture locks.

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

Like every Cargo alias, `cargo stand` may compile its binary before it can
print the plan. That Cargo startup is not an installer plan step. Missing helper
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
| Bluetooth helper `check` | Uses the finite helper grant and prints its report | No | Yes, with restoration | Yes, bounded DTM TX | No |
| `cargo hil fixture check <scenario>` | Writes a local report; checks scenario prerequisites | No provider actions in the current registry | No | No | No |
| `run` / `run-all` | Builds and writes evidence; uses selected fixture grants | Depends on scenario | Depends on scenario | Yes for radio scenarios | Yes |

The installed helpers' finite interfaces, capability versions and runtime
effects are documented beside [Linux network](../linux-net/README.md) and
[Bluetooth](../linux-bluetooth/README.md) fixtures. A software installation
receipt records a consistent installation; runtime and hardware observations
are separate from it.

`cargo stand fixture build-hostapd` builds the pinned hostapd release and
reviewed [coexistence patch](../linux-net/hostapd/README.md) without root.
`cargo stand fixture build-udhcpc` builds the pinned DHCP client. The network
installer uses these same builds and their provenance.
