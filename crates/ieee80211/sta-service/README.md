# Wi-Fi station drivers

`oer-ieee80211-sta-service` drives the station state machines of
[`oer-ieee80211-sta`](../../../protocols/ieee80211/sta/README.md). That crate
is sans-IO: it owns state, policy and the ports; this crate owns the futures
that wait on those ports and on the `oer-time` `Timer`, for any executor.

| Module | Driver |
| --- | --- |
| `join` | `StaJoinRunner`: Open System and SAE Authentication and Association against absolute millisecond deadlines |
| `scan` | `StaCandidateScanService`: one finite channel plan and candidate selection; `StaScanBackend`: the channel-visit transaction (switch, receive, optional probe, dwell, stop) over a `StaScanPort` |
| `attempt` | `StaAttempt`: every pre-connected phase of one attempt over a `StaAttemptPort`, once and in order |
| `station` | `StaLifecycleService`: outer attempt, reconnect, backoff, disconnect and stop |

Every driver returns the exact caller-owned radio state at each success,
retry, stop and failure edge. The ESP32-S31 runtime implements the scan and
attempt ports with its chip owners and polls these drivers.
