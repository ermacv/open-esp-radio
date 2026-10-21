# w3 report contract

W3 joins the laboratory router's open 2.4 GHz HT20 BSS, obtains an IPv4
lease by DHCP, and exchanges 100 ICMP echo requests in each direction at
one-second intervals. Every echo must receive a reply. The router profile
disables HE and uses a generated SSID for each run.

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
The router capture also checks request cadence against a one-second
schedule, allowing 250 ms of arrival jitter per request relative to the
first. An image's `interval_ms` claim alone cannot establish cadence.

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
