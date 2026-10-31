# HIL scenarios

A HIL scenario tests the device under test from the host. The HIL core
(`protocols/hil`, `tools/hil`) only carries data and commands; everything a
scenario knows lives here or beside the facade it uses.

| Directory | Contents |
| --- | --- |
| `images/` | RPC images: a product facade or engine, the HIL agent and the handlers of the services the image mounts. Several scenarios share one image. |
| `services/` | Test-only service vocabularies: requests, responses and, where it is generic, the handler over a product contract. A product facade's vocabulary lives beside the facade. |
| `<scenario>/` | One host program per scenario: its cases, its independent reference and its verdict, over `oer-hil-harness`. |

| Image | Services |
| --- | --- |
| [`esp32s31-rpc`](images/esp32s31-rpc/README.md) | AES |
| [`esp32s31-wifi-station`](images/esp32s31-wifi-station/README.md) | Wi-Fi station: join, status and leave over the product station facade, IPv4 by DHCP; UDP load and executor CPU |

| Scenario | Image | Checks |
| --- | --- | --- |
| [`aes`](aes/README.md) | [`esp32s31-rpc`](images/esp32s31-rpc/README.md) | The product hardware RFC 3394 key unwrap against the RFC's known answer and an independent host wrap |
| [`wifi-load`](wifi-load/README.md) | [`esp32s31-wifi-station`](images/esp32s31-wifi-station/README.md) | UDP load between the host and the station through the lab router, RX, TX and both, up to 100 Mbit/s offered: throughput, loss, reordering, RTT and CPU, reported only |
| [`wifi-station`](wifi-station/README.md) | [`esp32s31-wifi-station`](images/esp32s31-wifi-station/README.md) | The product Wi-Fi station joins the lab router's WPA2 network, takes a DHCP address, answers the router's pings and leaves, each checked on the router |

Equipment a scenario needs besides the board belongs to the stand
(`tools/lab`): a scenario that uses the lab router declares it
(`Scenario::using_router`) and runs its operations through `Cx::router`.

A case records measurements through `Cx::measure`: each is printed, and a
run with measurements writes them with its outcomes to
`target/hil/results/<scenario>-<time in ns>.json` after the lease is released,
with the cleanup and the verdict.
