# w2

An active 2.4 GHz scan followed by an acknowledged unicast Probe Request to
the stand AP. The image drives the S31 driver directly on the radio
executor (P2); the MAC, WDEVPWR and doorbell handlers run at P3. One task
consumes RX and TX facts. Before each actor retune it stops the receive
walker, drains frame leases and returns interrupt ownership; it reattaches
that ownership before restarting receive.

```sh
cargo fw run w2 --board s31-a
```

The host applies the stand router profile and reads its UCI country, AP
channel and BSSID. An absent or unsupported country refuses the run. These
values, the stand SSID and the lab's explicit 10 dBm cap configure only
this build. `cargo fw build w2` produces an unconfigured image that reports
`OER-RESULT: fail w2 unconfigured`. eFuse is read only, including the
station address; identities in console reports are masked.

For every permitted channel 1–13, Allowed channels get one broadcast probe
at the band's derived management rate followed by a 150 ms dwell; NO_IR
channels receive only. The scanner then returns to the lab channel and
sends a unicast probe with A1 = A3 = lab BSSID. Its rate comes from the
received BSS basic rate set; up to seven attempts use that same rate. One
management sequence counter covers the scan and unicast. The placed TX
pool uses the voice queue and the coexistence owner's probe priorities.
The legacy TX owner excludes HT/HE publication from this IRQ image.

The calibrated PHY power profile is capped by the minimum of country EIRP
minus the board's 2.33 dBi antenna gain, chip maximum and lab cap. Each
transmission reports all four per-queue power codes read back through
generated fields. BSS-color events are counted separately from queue
collisions and remain masked.

`OER-W2-BEGIN v=1` through `OER-W2-END lines=<n>` records each visited
channel, sequence, rate, retune duration, response count and power codes,
then the unicast outcome, counters, free IRQ-stack bytes and gates a–e.
`cargo fw` strictly parses the block and recomputes the gates: discovery
through a response on the lab channel, broadcast policy, acknowledged
unicast with a response, zero timeouts/collisions/service errors, at least
4096 free IRQ-stack bytes and power codes within the computed ceiling.

The host also holds the laptop AX211 monitor lease on the lab channel.
`dumpcap` streams a bounded capture into RAM; `tshark` decodes it from
stdin. No raw capture is written to disk. The host correlates the target's
reported sequence numbers, derived rates and Retry bits with broadcast
and unicast probes, an AP ACK and probe responses. Sequence correlation
also distinguishes adjacent-channel probes heard by the monitor: its
radiotap frequency describes the observer's channel. NO_IR policy is
checked against the channel/sequence plan; a country with no NO_IR channels
reports zero such channels. The managed laptop interface is restored
before releasing its lease.

Exit status is 0 when image, recomputation and air evidence pass, 1 for a
gate failure, 2 for a console timeout, and 3 for a malformed report or a
contradiction. Decoded console evidence is retained under
`target/fw/runs/<time>-w2/`.
