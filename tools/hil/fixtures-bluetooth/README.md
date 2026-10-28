# Linux Bluetooth fixture library

`oer-hil-fixtures-bluetooth` is the host-side Bluetooth fixture library.
It has no normal workspace consumers. The current HIL command tree exposes
`fixture check SCENARIO`; it does not expose the library's Bluetooth checks
as a CLI subcommand.

| API or module | Role |
| --- | --- |
| `check`, `run_in` | Finite DTM check through the installed `open-radio-bluetooth` helper, with typed reports and restoration checks |
| `att` | Owned kernel ATT connection and socket operations |
| `att_parameters` | Fixed ATT timing lease and restoration |
| `discovery` | BlueZ discovery and public peer identity |
| `secure_gatt` | D-Bus agent, pairing and GATT client lifetime |
| `model` | Re-export of the shared helper contract in `oer-lab-fixture::bluetooth::model` |

The finite helper is a binary of `tools/hil/fixture`; installation and fixed
launchers belong to `tools/lab/fixture-install`, with their shared contract
in `tools/lab/fixture`. See [installation](../run/fixtures.md#linux-fixture-software-installation)
and the [helper interface](../linux-bluetooth/README.md). Library tests use
synthetic inputs and do not exercise an adapter.

```console
cargo test -p oer-hil-fixtures-bluetooth
```
