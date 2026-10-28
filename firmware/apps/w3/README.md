# w3 report contract

The build selects the configured composition only when all five stand
settings are present. Without them, a separate entry adopts the platform,
prints `OER-RESULT: fail w3 unconfigured` and idles with no tasks or interrupt
bindings. Only the configured composition declares executor task poll records.

The image composes the S31 lower-MAC backend, channel arbiter, open
`StationService` and Espressif rate selection with the owned Embassy network
adapter. P2 owns the device and port registration; the station and IP stack run
on P1 in PSRAM. The linker sizes this image's P1 stack under `stack.toml`, with the platform's
unchanged interrupt-stack allocation. The image stores the large station future
in static PSRAM using the same pinned compiler support as the HIL images.
The image declares its P1 stack proven: unresolved calls fail the build.
The accepted bound is at most 42,192 bytes; the linker adds the 16,384-byte
minimum-free reserve to that proven bound.

Outgoing ICMP uses the pinned fork's raw IPv4 socket and checked wire codecs;
incoming requests use native `icmp-ping-reply`. The image validates observations
before reading echo identities, counts malformed packets and queue drops, and
checks each adjacent outgoing request against the 750–1250 ms cadence window.
The bridge checks the owned endpoint's RX publisher readiness before removing
an RX input. Each turn drains ready RX frames up to the RX input
queue capacity and advances TX owners up to the TX input queue capacity, then
yields. Both output readiness checks precede their input dequeue, so a full
output retains the source owner. RX pool exhaustion waits for capacity while
TX remains eligible to return those same pool owners. After a productive
bounded step the bridge yields; idle waits use readiness wakes and the
existing one-millisecond link timer. The synchronous drain step stays out of
the generated poll frame with `#[inline(never)]`; the loop has one await.
The packet ownership and copy boundaries are unchanged.

A local probe socket error or its 250 ms send deadline consumes that probe
and is counted separately, without marking the link failed or ending the
series early. The next probe keeps its one-second schedule. Nonzero counters
and the last socket error appear in a `w3.probes` diagnostic before the
unchanged version 1 console block; missing probes still fail gate c. The
normal final deauthentication and gate f remain required. A `w3.bridge`
diagnostic records whole-workload turns (including empty RX phases), total RX
frames dequeued and the maximum RX batch; the host derives the mean from the
sum and turn count. `w3.queue` samples the service input occupancy once per
bridge turn (including empty turns): `samples`, `sum`, `max`, turns with a
ready input but blocked publisher (`publisher_blocked`), and successful
publications (`published`). Its mean is `sum / samples`, not the mean batch
size. `w3.echo` counts all router-to-station echo requests seen by the bridge
and replies admitted to the service TX queue, independent of the version-1
100-pair window. `w3.stack` snapshots whole-stack RX owners, validated IPv4
echo requests, formed echo replies, raw-egress TX attempts/refusals and refused
IPv4 echo replies. Counters saturate and never change admission or retry.
Probe, bridge and loss diagnostics are typed notes like the
version 1 report and do not format text on the target.

It uses no power save, hardware AID or fixed PHY rate. A software beacon-loss
deadline disconnects after six seconds without a matching BSS beacon.

```sh
cargo fw run w3 --board s31-a
```

The additional `--load-1ms` mode uses the same leases, generated W3 AP profile
and single configured build. The host prepares a temporary, hash-checked
OpenWrt fping 5.3 extraction without installation; the router sends the #486
56-byte, 1 ms × 10,000-request flood after two consecutive 5/5 readiness
batches. Recovery batches are 5/5 at +0/1/5/15/30 s. A ready AP tcpdump stream
covers the complete echo workload (ICMP types 0/8) and must report zero kernel
drops. Its original numeric text and capture-health flags are retained even
if decoding fails. The load decoder accepts tcpdump's empty stop separator;
the standard observation decoder retains its strict row contract.
The mode records its own load result and a loss table in `report.json`; the
ordinary version-1 console block remains unchanged. Flood/recovery identities
are separate from its 100-pair peer gate. All AP-initiated echoes, including
warmup and recovery, are conserved across the bridge, stack, TX refusal,
service TX and AP reply boundaries; the flood is also counted independently
by identifier. A mismatch refuses the accounting instead of assigning losses.
The AP-to-bridge residual includes service queue-full and any earlier loss;
the aggregate service queue-full count also includes non-ICMP traffic.
The station service keeps no counters: the `w3.rx` row (service queue-full,
rejected frames, their reasons and lost port events) counts the service's
observation events in the image's collector, so W3 declares the `trace`
instrumentation class and does not build without it.
The preflash load gate requires a complete P1 bound <=42,192 bytes and zero
unresolved sites; memory growth is reported. The source extraction is removed
before releasing the lease. Loss measurements use AP evidence and make no
claim about an independent over-the-air observation.

```sh
cargo fw run w3 --board s31-b --load-1ms
```

The command obtains exclusive board, router and air ownership, applies the
generated W3 profile, builds and flashes its configured image, and recomputes
gates from the console and independent router evidence. Decoded reports,
observations and the masked console are saved in `target/fw/w3-runs/<run-id>/`.
The separate final lower-MAC conformance run supplies gate e.

W3 joins the laboratory router's open 2.4 GHz HT20 BSS, obtains an IPv4
lease by DHCP, and exchanges 100 ICMP echo requests in each direction at
one-second intervals. Every echo must receive a reply. The router profile
disables HE and uses a generated SSID for each run.
Its dedicated W3 bridge has no LAN/WAN ports or forwarding. OpenWrt serves DHCP
on `10.231.0.0/24`, and both ping directions use its `10.231.0.1` address.
The [profile settings](../../../tools/lab/router/README.md#w3-open-station)
enumerate the network, DHCP, firewall and wireless changes, applied without
restoration. The LAN interface and management address are preserved.

The host observes the target's association, DHCP lease, ICMP traffic and
departure independently on the router. The final firmware report supplies
target-side association, DHCP and ping results, TX timeout and queue
collision counters, and free interrupt-stack bytes. Gate e is the separate
lower-MAC conformance scenario.

Console reports and saved host observations contain no SSID, passphrase,
SSH target, interface name or full MAC address. Addresses are shown as
`xx:xx:xx:xx:HH:HH`.

## Console block, version 1

The image prints one complete block in this order. All twelve body rows,
all fields and the final result marker are mandatory, including on failure.
Numbers are unsigned decimal. Booleans are `0` or `1`. Identities use the
uppercase masked form. SSIDs and full identities never occur in a row.
Diagnostics can precede the block; the block and result are contiguous.

```text
OER-W3-BEGIN v=1
w3.config channel=6 target=xx:xx:xx:xx:00:08 stand=xx:xx:xx:xx:00:01
w3.association authenticated=1 associated=1
w3.lease acquired=1 address=192.0.2.8 router=192.0.2.1 prefix=24
w3.ping sent=100 received=100 interval_ms=1000
w3.peer_ping requests=100 replies=100
w3.stats tx_timeout=0 queue_collision=0 irq_free=4096
w3.departure deauth_sent=1
w3.gate id=a pass=1
w3.gate id=b pass=1
w3.gate id=c pass=1
w3.gate id=d pass=1
w3.gate id=f pass=1
OER-W3-END lines=12
OER-RESULT: pass w3
```

`target` is the station MAC; `stand` is the selected AP BSSID. Association
records successful open authentication and association separately. DHCP
records the actual assigned address, prefix length and DHCP router. Without
a lease, the row is `acquired=0 address=0.0.0.0 router=0.0.0.0 prefix=0`.
`w3.ping` counts this station's transmitted echo requests and distinct
matching replies; `w3.peer_ping` counts the router's distinct echo requests
and the station's replies. Each ping direction uses one identifier and 100
distinct sequence numbers. Lost replies, duplicates substituted for missing
sequences and replies preceding their requests cannot pass.
The router capture checks each pair of adjacent echo requests for an interval
of 1 s ± 250 ms, in both directions. There is no absolute phase requirement
relative to the first request and no separate series-duration gate. An image's
`interval_ms` claim alone cannot establish cadence.

`tx_timeout` and `queue_collision` cover the entire run through final
deauthentication. `irq_free` is the minimum free interrupt-stack bytes,
reported without an additional W3 acceptance threshold. `deauth_sent`
records transmission of the target's final deauthentication. The image
continues serving incoming ICMP while its own ping is running. It allows a
10-second observation setup window after DHCP before sending its first
echo, then serves the complete bidirectional exchange before departure.

The host subscribes to router disconnect events and starts an ICMP capture
before the first echo. It starts its own `ping -c 100 -i 1` after DHCP and
association are visible. The station remains available until both 100-echo
exchanges finish, with a bounded failure deadline, then deauthenticates.
The host checks the AP's target station entry is gone and observes a new
`AP-STA-DISCONNECTED` event for that target. A station entry disappearing
alone cannot establish departure.

## Recomputed gates

| Gate | Required evidence |
| --- | --- |
| a | Image authenticated and associated; router target entry is present, authenticated, associated and authorized |
| b | Image DHCP lease matches the router's unique, unexpired MAC-selected lease and router IPv4 address |
| c | Image sent/received 100/100 at 1000 ms; router observed all 100 distinct station-to-router echo pairs |
| d | Router ping sent/received 100/100 at 1000 ms; image served 100 requests/replies; router capture contains all 100 reverse echo pairs |
| f | Zero image TX timeouts and queue collisions, image deauth sent, a new router disconnect event, and target absent at the end |

All gates require the run's verified open HT20 profile with HE disabled.
The router must see the target absent before the run's association. The
image's gate claims and `OER-RESULT` must equal the host's recomputation.
`OER-RESULT: fail w3` accompanies any failed gate. Gate e is evaluated by
the existing `lower-mac-conformance` scenario, outside this report.

```sh
cargo fw w3 check --console <console.txt> --observations <router.json>
```

`check` exits 0 for pass, 1 for an agreed gate failure and 3 for malformed
input or a contradiction. Repeated/unknown fields, rows or blocks,
unsupported versions, invalid counts, truncation and trailing protocol
garbage are errors. Router evidence is schema 1 of
`oer_lab_router::w3::Evidence`, with strict JSON fields. Its ICMP rows contain
only numeric timestamps, IPv4 endpoints, echo kind, identifier and sequence.

AX211 station verification uses the installed `linux-net` fixture through
its launcher and finite sudo policy, under router, physical-radio and
exclusive air ownership. NetworkManager's unmanaged rule stays enabled.
The installed fixture must provide schema 16: open authentication, a station
DHCP client with lease readback and a bounded 100-echo workload with decoded
counts. Run the host acceptance with:

```sh
cargo fw w3 ax211
```

The command waits for the router, physical-radio, fixture-software and
exclusive-air leases. It applies the profile, observes association and DHCP,
captures both 100-echo exchanges on the router and observes final departure.
It saves a decoded `report.json` under `target/fw/w3-ax211/<run-id>/`, including
source and installed-generation identities and separate workload/cleanup errors.
Its five checks are association, lease, station ping, router ping and departure.
Firmware TX counters and IRQ stack measurements require the image run; the
AX211 report has the distinct scope `linux-net-host-acceptance`.
See [fixture installation and ownership](../../../tools/hil/run/fixtures.md).
