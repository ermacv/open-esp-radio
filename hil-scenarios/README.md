# HIL scenarios

A HIL scenario tests the device under test from the host. The HIL core
(`protocols/hil`, `tools/hil`) only carries data and commands; everything a
scenario knows lives here or beside the facade it uses.

| Directory | Contents |
| --- | --- |
| `images/` | RPC images: a product facade or engine, the HIL agent and the handlers of the services the image mounts. Several scenarios share one image. |
| `services/` | Test-only service vocabularies: requests, responses, `messages.lock` and, where it is generic, the handler over a product contract. A product facade's vocabulary lives beside the facade. |
| `<scenario>/` | One host program per scenario: its cases, its independent reference and its verdict, over `oer-hil-harness`. |

| Scenario | Image | Checks |
| --- | --- | --- |
| [`aes`](aes/README.md) | [`esp32s31-rpc`](images/esp32s31-rpc/README.md) | The product hardware RFC 3394 key unwrap against the RFC's known answer and an independent host wrap |

Equipment a scenario needs besides the board belongs to the stand
(`tools/lab`).
