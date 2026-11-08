# Wi-Fi station scenario

`cargo hil run wifi-station` checks the Wi-Fi station of the application
entry `oer` (its station and network core, IPv4 over DHCP) on the
`hil-wifi-station` image through the
[Wi-Fi station service](../../services/wifi-station). Every
claim of the device is checked against the lab router, which the run leases
with the board and the air (`Scenario::using_router`).

| Case | Checks |
| --- | --- |
| `join` | The router brings up its isolated station network with a generated SSID and passphrase (WPA2-Personal, CCMP) on the first channel the stand file's `[router]` allows; the device joins it, linked on that channel, and the router's station table shows it authenticated, associated and authorized |
| `dhcp` | The device takes an IPv4 address by DHCP; the router's lease for its MAC holds that address, and the router (`10.231.0.1`) is its gateway |
| `ping` | The router pings the device's address ten times; at least nine replies |
| `restart` | A refused publication restarts the station: the same boot counts one restart whose cause is that refusal, and its reconnect policy brings the device back with no command from the host; at least nine of ten pings answered |
| `leave` | The device leaves: its link is down, and the router's station table no longer lists it |

The cases run in order on one boot: each needs what the one before did.
After the run the LAB service disables the router's AP section again.

```console
cargo test -p oer-hil-scenarios-wifi-station
```

## Connect report

After the join, answered or not, the run prints lines starting `connect:`:
when each stage after the scan began, in milliseconds from the connect's
start, then one row per channel visit. A row's columns are the time from
asking for the channel to the grant, to the port on the channel, to the
receive rules, to the probe handed over, then listening, the whole time on
the channel, the time from reaching the channel to the first frame heard and
to the first of the network, the frames heard, the network's beacons and probe
responses, its strongest level, the probe responses addressed to the station
and those overheard, and the probe's state. Times are when the station handled
a frame, not when it was received, and the beacons and `resp` count the
connect's own network. The probe's state is `built` until the port ends it,
then `sent` (the port counted a transmission), `unsent` (it reports none) or
`unknown` (it does not establish one); these are the port's report, not the
air. `answer` is a probe response addressed to the station that reached the
scan, and `timeout` a sent probe whose visit ended with none. A visit that
built no probe shows `-`. `dhcp` prints the time to the
address from the join's start.
