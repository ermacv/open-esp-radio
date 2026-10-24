# Wi-Fi HIL family

`w4` qualifies the ESP32-S31 WPA2-Personal station image. The host derives
gates a–f from independent router ICMP/DHCP/link observations, typed target
counts and a passive laptop capture. Air metadata retains association,
four-way message numbers/replay counters, frame protection and public KeyIDs;
raw pcap and EAPOL key data stay in bounded memory. An empty capture, missing
M4 or missing GTK change fails with a reason.

The independent `lower-mac-conformance` run supplies key cases. Select that
exact completed run explicitly; the reader checks the same device, decodes its
preparation and hardware traces, then regenerates the reference model. A run
verdict is never a substitute for those traces. Its run ID is retained in the
W4 observations. There is no search for a previous successful run.

```sh
OER_W4_KEY_CONFORMANCE_RUN=RUN_ID cargo hil run w4 --board s31-a --owner NAME
```

The run holds the board, router, passive laptop monitor and exclusive air.
The managed router must provide native `ip neigh` and `tcpdump`; the run checks
these before configuring the router and never installs fixture software.
It generates the SSID/passphrase, applies WPA2-PSK/CCMP HT20 without HE/PMF,
then sends `StartStation` at run time. The 100-echo window spans GTK replacement;
the tool observes the station's neighbor entry, flushes it (`ip neigh flush to ADDRESS dev IFACE`, as BusyBox `ip` has no `del`) if present and
confirms absence. An already absent entry is recorded as the required state;
a failed flush passes only if the confirming read observes absence. The
next measured router ping triggers a kernel broadcast ARP. Gate c requires the decoded request
on the new GTK and the station's ARP reply, each matched to its protected air
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
neither command success nor absence alone replaces gate c's frame evidence.
PTK rekey and SAE are outside this workload.

`w4-initial-gtk` is a separate investigation with group rekey disabled. It
first requires captured AP beacons on the configured channel, then records
initial-key DHCP, protected air metadata, fresh hostapd events and typed RX
rejections. It stops the station on success or failure and makes no W4 claim.
The qualification profile retains its 30-second GTK interval.
