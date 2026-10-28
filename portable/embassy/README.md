# Embassy bindings

These crates bind Embassy facilities to driver contracts. Chip radio
execution belongs to the [radio runtime domain](../../chips/espressif/executor/README.md), which
holds no Bluetooth runtime.

| Location | Responsibility |
| --- | --- |
| `net/` | Owned-packet boundary between the Embassy/Xarxa network stack and the radio: the software/physical TX ownership contracts and `TxQueues` (`datapath`) |
| `time/` | `EmbassyClock`, the `oer_time` clock over Embassy time |

An adapter can retain state required by its external contract.

[`exec/executor`](../../chips/espressif/executor/) is the Embassy platform binding:
executor wake-up and timer-queue ABI (`src/{executor,time_driver}.rs`);
applications supply interrupt and timer capabilities. It has no radio policy
or PHY initialization. Image time is [`time`](time/)'s `EmbassyClock`, which
chip PHY and the runtimes take as an `oer_time::Timer`; chip PHY remains
executor-independent.

Final memory profiles, static claims, IRQ binding and whole-radio lifecycles
belong to the integrating firmware (`firmware/apps`). Station network ownership
across associations is `net/`'s `station_network`; finite task-group shutdown is
[`oer-cooperate`](../cooperate/)'s `connected_tasks`; monitor handoffs are runtime
code in [`ieee80211/runtime`](../../protocols/ieee80211/runtime/). None of them is an
executor binding. An executor-free service never depends on an adapter. The
[architecture](../../docs/architecture.md) defines the ownership direction.
