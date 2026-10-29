# Wi-Fi HIL family

`wifi-station` qualifies the ESP32-S31 WPA2-Personal station image. The host derives
station lifecycle, four-way handshake, group key change, protected air traffic,
reconnect and key conformance gates from independent router ICMP/DHCP/link
observations, typed target counts and a passive laptop capture. Air metadata
retains association,
four-way message numbers/replay counters, frame protection and public KeyIDs;
raw pcap and EAPOL key data stay in bounded memory. An empty capture, missing
M4 or missing GTK change fails with a reason.

The independent `lower-mac-conformance` run supplies key cases. Select that
exact completed run explicitly; the reader checks the same device, decodes its
preparation and hardware traces, then regenerates the reference model. A run
verdict is never a substitute for those traces. Its run ID is retained in the
Wi-Fi station observations. There is no search for a previous successful run.

```sh
OER_WIFI_STATION_KEY_CONFORMANCE_RUN=RUN_ID cargo hil run wifi-station --board s31-a --owner NAME
```

The run holds the board, router, passive laptop monitor and exclusive air.
The managed router must provide native `ip neigh` and `tcpdump`; the run checks
these before configuring the router and never installs fixture software.
It generates the SSID/passphrase, applies WPA2-PSK/CCMP HT20 without HE/PMF,
then sends `StartStation` at run time. The 100-echo window spans GTK replacement;
the tool observes the station's neighbor entry, flushes it (`ip neigh flush to ADDRESS dev IFACE`, as BusyBox `ip` has no `del`) if present and
confirms absence. An already absent entry is recorded as the required state;
a failed flush passes only if the confirming read observes absence. The
next measured router ping triggers a kernel broadcast ARP. The group key change
gate requires the decoded request on the new GTK and the station's ARP reply,
each matched to its protected air
frame by public Sequence Number, PN and KeyID. Missing either frame fails, as does a
reply captured before its request or any timestamp outside the 5 s window after the trigger;
capture and host clocks are not compared with each other.
After router deauthentication,
a fresh association/handshake and a ten-echo window precede final departure.
After an accepted StartStation, workload errors also request StopStation;
a cleanup failure is explicit and never replaces a gate with PASS.
Credentials and key material enter no scenario, environment, log or result.
Each router step records its command, exit code/signal and stderr, including
early observer shutdown and command failures. Generated credentials and stand
identities are redacted before serialization; configuration stdout is omitted.
The neighbor refresh records its before/after presence and whether deletion
was attempted. Command availability or a remaining neighbor is a failure;
neither command success nor absence alone replaces the group key change gate's
frame evidence.
PTK rekey and SAE are outside this workload.

The WPA2 station profile uses a 30-second GTK interval.

Traffic windows are peer-originated ping: 100 requests before departure and ten
after reconnect. The DUT uses ordinary stack echo replies. Each result compares
peer ping counts with an independent complete, loss-free ICMP capture, ordered
identities and one-second request pacing (0.75–1.25 seconds). The 100-echo
window checks all identities and replies, including those queued while the
scenario flushes the neighbor after GTK replacement. Its pacing measurement
starts after the first ICMP reply following the exact, protected ARP reply
on the new GTK; at least two subsequent requests are required. The boundary
is derived from the independent ICMP capture and exact air evidence, and is
recorded as `wifi-station-icmp-pacing-start-us`. The ten-echo reconnect window
retains pacing across all its requests. No `ProbeStation` command or target
traffic counters participate. Product readiness is checked with `GetStationStatus`;
trace collectors still supply GTK, ARP and radio observations to the existing gates.

Native ping output is parsed on the host. Each window retains every reply's
sequence, RTT in microseconds and duplicate marker in `observations.json` under
`wifi-station.ping.group-key-change` or `wifi-station.ping.after-reconnect`.
Lost sequences have no entry; duplicate replies remain separate entries.
Reported min/avg/max/mdev become informational measurements named
`wifi-station.ping.<window>.rtt-us.<statistic>`, with no direction, threshold or
verdict. Decimal milliseconds are converted exactly to integer microseconds.
A missing RTT summary produces no summary measurements; BusyBox's min/avg/max
summary has no mdev. Neither is synthesized from the reply list. Completed ping
observations survive later GTK, capture or acceptance failures.

Scenario, workload, profile and acceptance gate names describe the behavior they exercise.

The `station-lifecycle` gate covers association, DHCP, the 100-echo window,
final departure and error counters. `four-way-handshake`, `group-key-change`,
`protected-air-traffic`, `reconnect` and `key-conformance` retain their independent
evidence requirements and appear under these names in output and results.
