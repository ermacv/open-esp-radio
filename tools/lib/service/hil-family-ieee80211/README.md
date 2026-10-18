# Wi-Fi HIL family

`oer-hil-family-ieee80211` runs the `[wifi]` scenarios: station, access
point and monitor roles and the station's UDP, TCP and ICMP traffic. Three
crates split the family by owner:

- this crate: the scenario table, the workloads and the Wi-Fi exchanges with
  the target (`link::WifiCapture`);
- `oer-hil-family-ieee80211-fixture`: the access points, monitors and host
  routes a workload runs against, prepared by its `FixtureProvider`;
- `oer-hil-family-ieee80211-evidence`: analysis of what the host observed on
  the air, independent of the target.

Host traffic comes from `oer-hil-net-traffic`: network sessions, paced UDP
and TCP and the one ICMP method, datagram sockets bound to the fixture
interface. Each workload records typed observations (`rx`, `tx`, `tcp`,
`bidirectional`, `icmp`, …) in the repetition's `observations.json`; no
workload writes a Markdown report.

```console
cargo test -p oer-hil-family-ieee80211 -p oer-hil-family-ieee80211-fixture \
  -p oer-hil-family-ieee80211-evidence
```
