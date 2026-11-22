# HIL scenarios

A HIL scenario tests the devices under test from the host. The HIL core
(`protocols/hil`, `tools/hil`) only carries data and commands; everything a
scenario knows lives here or beside the facade it uses.

A probe differs from a scenario only in what it is allowed to use. A probe may use an API of any level, `unsafe` and any other means it needs; a scenario uses only the public API. The work between the DUT and the host is done through the HIL protocol and the agent.

| Directory | Contents |
| --- | --- |
| `images/` | RPC images on the `oer` application entry: the HIL agent and the handlers of the services the image mounts. Several scenarios share one image. An image builds the same `boot` as the example of its function and differs only by features, explicit settings and its handlers. |
| `probes/` | Images below the entry. A probe whose manifest declares `console = "hil"` serves its services through the same agent; the others still print console lines ending in `OER-RESULT:`. |
| `services/` | Test-only service vocabularies: requests, responses and, where it is generic, the handler over a product contract. A product facade's vocabulary lives beside the facade. |
| [`agent`](agent/README.md) | The HIL agent: what a probe or an image on the `oer` entry serves besides its handlers, below the entry. |
| `<scenario>/` | One host program per scenario: its cases, its independent reference and its verdict, over `oer-tools-hil-harness`. |

| Image | Services |
| --- | --- |
| [`hil-aes`](images/aes/README.md) | AES |
| [`hil-wifi-station`](images/wifi-station/README.md) (outside the build) | SDK station: connect, address, link loss and status over the `oer` station and its network core, which has no TCP: an exchange is refused; Wi-Fi station: join, status and leave over the same station; fault: an exception raised on request; load: UDP load on the station's address and the library's observations of its work |

The images and scenarios marked "outside the build" stand on the old Wi-Fi
path and stay as sources ([repository layout](../docs/repository-layout.md));
`sdk-station` and `wifi-access-point` build, and their images do not.

| Scenario | Image | Checks |
| --- | --- | --- |
| [`aes`](scenarios/aes/README.md) | [`hil-aes`](images/aes/README.md) | The product hardware RFC 3394 key unwrap against the RFC's known answer and an independent host wrap |
| [`radio-pair`](scenarios/radio-pair/README.md) | the probes [`esp32s31-rx-ring`](probes/esp32s31-rx-ring/README.md) (receiver) and [`esp32s31-case-c-sender`](probes/esp32s31-case-c-sender/README.md) (sender) | The receive ring's cases R1 to R4 over case C's frames, each boot's snapshots and receive counters kept for `radio-pair-compare` |
| [`sdk-station`](scenarios/sdk-station/README.md) | [`hil-wifi-station`](images/wifi-station/README.md) | The `oer` station on the lab router's WPA2 network: a wrong passphrase is a handled failure without a reset, it joins, takes a DHCP address, exchanges a line over TCP with the host, rejoins after a deauthentication, and its address wait times out while the router serves no DHCP; an exception raised on request is recorded, reset and served once by the next boot |
| [`wifi-load`](scenarios/wifi-load/README.md) (outside the build) | [`hil-wifi-station`](images/wifi-station/README.md) | UDP load between the host and the station through the lab router, RX, TX and both, up to 100 Mbit/s offered: throughput, loss, reordering, RTT and CPU, reported only |
| [`wifi-station`](scenarios/wifi-station/README.md) (outside the build) | [`hil-wifi-station`](images/wifi-station/README.md) | The `oer` Wi-Fi station joins the lab router's WPA2 network, takes a DHCP address, answers the router's pings and leaves, each checked on the router |

A scenario of several devices names each by its role
(`Scenario::with_roles`, `Role`); its program runs with `--board
<role>=<board>` for each, and a case reaches each through `Cx::device`.
`Cx`'s own requests go to the first role. A case that works with one
device while another runs a long request posts it (`DeviceCx::post`) and
takes its answer later (`DeviceCx::wait`); until then that device takes no
other request.

Equipment a scenario needs besides its boards belongs to the stand
(`tools/lab`): a scenario that uses the lab router declares it
(`Scenario::using_router`) and runs its operations through `Cx::router`.

A case records measurements through `Cx::measure`: each is printed, and a
run with measurements writes them with its outcomes to
`target/hil/results/<scenario>-<time in ns>.json` after the lease is released,
with the cleanup and the verdict.
