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
The managed router must provide native `ip neigh` and `tcpdump`; admission checks these
before execution and never installs fixture software during qualification.
It generates the SSID/passphrase, applies WPA2-PSK/CCMP HT20 without HE/PMF,
then sends `StartStation` at run time. The 100-echo window spans GTK replacement;
the tool deletes only the station's neighbor entry and the next measured
router ping triggers a kernel broadcast ARP. Gate c requires the decoded request
on the new GTK and the station's ARP reply, each matched to its protected air
frame by public Sequence Number, PN and KeyID. Missing either frame fails.
After router deauthentication,
a fresh association/handshake and a ten-echo window precede final departure.
Credentials and key material enter no scenario, environment, log or result.
PTK rekey and SAE are outside this workload.
