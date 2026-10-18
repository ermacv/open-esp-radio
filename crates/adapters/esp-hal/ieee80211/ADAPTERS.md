# ESP32-S31 bindings to esp-hal

These modules connect the driver's typed contracts to the pinned `esp-hal`
fork and upstream `esp-pacs`. They own platform integration;
portable radio policy and the generated radio PAC live elsewhere.

| Package directory | Responsibility |
| --- | --- |
| [soc](soc/README.md) | Upstream SoC register operations, cache/MMU, GDMA descriptors and transfer/completion ownership |
| [radio](radio/README.md) | Shared radio platform lease, clocks, interrupt publication and routing |
| `ieee80211` | Wi-Fi peripheral binding used by concrete composition |

The radio SVD, raw accessor backend and restricted semantic capability catalog
have their own [PAC provenance](../../../hardware/esp32s31/pac/README.md). The
publisher belongs to host tooling. The `soc` adapter is the handwritten
`oer-esp32s31-soc-esp-hal` package.

Executor/time ABI bindings live under `adapters/embassy`; complete radio tasks
live under `runtime`; final static resources and board composition
belong to `composition`. A platform adapter does not become a second radio
owner when another protocol uses the shared hardware.

## Dependency boundary

Upstream `esp-pacs` publishes the Wi-Fi, Bluetooth and IEEE 802.15.4 interrupt
sources as `MODEM_*` vectors, so the platform PAC is not forked. Radio register fields and
their access policy belong to this repository's radio PAC. IEEE 802.15.4 uses
the ordinary typed HAL interrupt API; the adapter owns binding and teardown
on the same CPU.

The HAL fork extends upstream ESP32-S31 support with radio ownership tokens,
reference-counted shared modem clock gates, TRNG, 120 MHz flash timing,
external-memory startup and interrupt handoff mechanisms; the fork's
`UPSTREAM.md` lists the delta and the last merged upstream revision. The shared board profile in `platform/esp32s31`
chooses memory sizes and timings; the platform bootstrap owns relocation and the
transition to the separately linked runtime. HAL PSRAM adoption records an
existing mapping without resetting the device, remapping live memory or
reprogramming the PSRAM PHY supply.
`esp_hal::interrupt::interrupted_context()` returns the return address of the
code the running interrupt handler preempted and the address of the trap frame
the vector stub passes to the dispatcher. The preempted stack pointer belongs to
the entry that built that frame: the platform runtime's
`stacks::interrupted_stack_pointer()` reads it from its own frame. The HIL hang
watchdog reports both harts' stalled context with them.
S31 Ethernet is enabled explicitly through HAL's `__ethernet` feature, so a
radio build does not acquire its network-driver dependencies. The Ethernet implementation remains available in the fork.

Cargo manifests pin immutable dependency revisions, and each workspace lockfile
records the resolved graph. All workspace islands use the same S31 PAC
revision. A source check establishes API compatibility; PSRAM execution,
interrupt placement and multicore startup also require HIL checks.
