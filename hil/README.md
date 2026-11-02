# HIL scenarios

A HIL scenario tests the device under test from the host. The HIL core
(`protocols/hil`, `tools/hil`) only carries data and commands; everything a
scenario knows lives here or beside the facade it uses.

| Directory | Contents |
| --- | --- |
| `images/` | RPC images on the `oer` application entry: the HIL agent on the entry and the handlers of the services the image mounts. Several scenarios share one image. An image builds the same `boot` as the example of its function and differs only by features, explicit settings and its handlers. |
| `services/` | Test-only service vocabularies: requests, responses and, where it is generic, the handler over a product contract. A product facade's vocabulary lives beside the facade. |
| [`hil-agent/espressif`](agent/README.md) | The HIL agent on the `oer` application entry: what an image built on the SDK serves besides its handlers. |
| `<scenario>/` | One host program per scenario: its cases, its independent reference and its verdict, over `oer-tools-hil-harness`. |

| Image | Services |
| --- | --- |
| [`hil-aes`](images/aes/README.md) | AES |
| [`hil-wifi-station`](images/wifi-station/README.md) | SDK station: connect, address, TCP exchange, link loss and status over the `oer` station and IP service; Wi-Fi station: join, status and leave over the same station; fault: an exception raised on request; load: UDP load on the station's address and the library's observations of its work |

| Scenario | Image | Checks |
| --- | --- | --- |
| [`aes`](scenarios/aes/README.md) | [`hil-aes`](images/aes/README.md) | The product hardware RFC 3394 key unwrap against the RFC's known answer and an independent host wrap |
| [`sdk-station`](scenarios/sdk-station/README.md) | [`hil-wifi-station`](images/wifi-station/README.md) | The `oer` station on the lab router's WPA2 network: a wrong passphrase is a handled failure without a reset, it joins, takes a DHCP address, exchanges a line over TCP with the host, rejoins after a deauthentication, and its address wait times out while the router serves no DHCP; an exception raised on request is recorded, reset and served once by the next boot |
| [`wifi-load`](scenarios/wifi-load/README.md) | [`hil-wifi-station`](images/wifi-station/README.md) | UDP load between the host and the station through the lab router, RX, TX and both, up to 100 Mbit/s offered: throughput, loss, reordering, RTT and CPU, reported only |
| [`wifi-station`](scenarios/wifi-station/README.md) | [`hil-wifi-station`](images/wifi-station/README.md) | The `oer` Wi-Fi station joins the lab router's WPA2 network, takes a DHCP address, answers the router's pings and leaves, each checked on the router |

Equipment a scenario needs besides the board belongs to the stand
(`tools/lab`): a scenario that uses the lab router declares it
(`Scenario::using_router`) and runs its operations through `Cx::router`.

A case records measurements through `Cx::measure`: each is printed, and a
run with measurements writes them with its outcomes to
`target/hil/results/<scenario>-<time in ns>.json` after the lease is released,
with the cleanup and the verdict.
