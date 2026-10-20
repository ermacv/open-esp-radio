# ESP32-S31 vendor contract reference

These documents describe pinned public-source and authenticated-artifact
contracts used by the vendor investigation. They explain hardware ordering,
software state, memory ownership and the limits of each observation. They are not run
results, work plans or product readiness declarations; the one exception,
[measurements](measurements.md), records on-chip values measured outside this
repository as data.

| Reference | Boundary |
| --- | --- |
| [Bluetooth controller](bluetooth-controller-boundary.md) | Platform lifecycle versus Controller software |
| [Bluetooth interrupts](bluetooth-interrupt-runtime.md) | Primary/NRT acknowledgement and deferred work |
| [Direct Test Mode](bluetooth-direct-test-mode.md) | Descriptor, timing, scheduler and recycle contracts |
| [Scheduler lists](bluetooth-scheduler-lists.md) | Multi-item lists, insertion into a live list and completion |
| [Legacy advertising](bluetooth-legacy-advertising.md) | PDU memory, event publication and recurrence |
| [Passive scanning](bluetooth-passive-scanning.md) | RX graph, completion and report fields |
| [Peripheral connection](bluetooth-peripheral-connection.md) | Connection memory, anchor timing and ownership |
| [IEEE 802.15.4 lifecycle](ieee802154-lifecycle.md) | Clock/reset, MAC foundation and reviewed register semantics |
| [IEEE 802.15.4 dataplane](ieee802154-dataplane.md) | Static policy, frame storage and IRQ ownership |
| [IEEE 802.15.4 control](ieee802154-control.md) | Ordered state transitions, STOP and timers |
| [IEEE 802.15.4 driver port](ieee802154-driver-port.md) | Complete open driver ledger, closed dependencies and ownership map |
| [Hardware measurements](measurements.md) | On-chip values whose records are outside this repository, conditions and status |

Platform notes record measured behaviour and techniques, with their
conditions and dates:

| Note | Subject |
| --- | --- |
| [Bring-up order](bring-up-order.md) | Wi-Fi PHY and coexistence before common power; the failed orders |
| [Core 1 sampling](core1-sampling.md) | Reading core 1's PC and SP from the debug-assist record |
| [CCA counter](phy/cca-counter.md) | Unresolved measurement-window ownership and limits of the caller analysis; no comparison exclusion |

[Registers](../../espressif/registers/README.md) owns reviewed hardware models
and publication; vendor artifact pins are in
[`vendor/artifacts.toml`](../vendor/artifacts.toml). Production behavior belongs
to the crates ([architecture](../../../docs/architecture.md)). Source hashes and artifact identities are
provenance constraints, not a claim that the implementation is equivalent.
