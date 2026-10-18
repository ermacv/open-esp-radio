# PHY trace

`oer-phy-trace` defines the trace points of the shared Espressif PHY domain
and its post-mortem snapshot as [`oer-trace`](../../../trace/src/lib.rs)
events. The target that emits them and the host that decodes them link the
same types, so no record is read as a bare number.

The types are chip-neutral. A chip's PHY maps its own refusals and failures
onto `Refusal` and `Fault`; `Fault::detail` keeps the chip's discriminant of
the failing step for comparing failures of one image.

| Channel | Event | Emitted at |
| --- | --- | --- |
| 0 | `Registration` | Cold registration start, calibration path, refusal or failure |
| 1 | `TrackingTick` | Every periodic tracking tick (1 Hz) |
| 2 | `RfLifecycle` | RF close after the last client and RF wake for the next |
| 3 | `ClientChange` | A protocol client joins or leaves the domain |
| 4 | `TemperatureReferences` | Tracking commits a temperature reference |
| 5 | `Poison` | The domain is poisoned and requires reset |
| 6 | `WifiChannel` | The domain tunes to a Wi-Fi channel |

The Domain::Phy range has twelve channels; 7 to 11 are unassigned. Each event
has its own channel, so a host can keep the tracking tick off while it
records lifecycle edges.

`record_poison` emits `Poison`, freezes the trace 16 entries later and
captures a `PhySnapshot` at point `phy.128`: the poisoning operation and
fault, the slot it started from, the active clients, the temperature
references, the references the radio held to each platform-owned clock
(saturating at 255, in the chip's platform clock order) and, when their
clock and power domain was on, the PBus and analog-I2C host busy flags and
the PBus result windows of the vendor `phy_pbus_rd` tables. The caller
decides from its own state whether that domain is on; otherwise the snapshot
records `BusRead::DomainOff` instead of reading a gated peripheral. The
function takes no lock and does not wait. Without the `oer-trace/record`
feature, `emit`, `freeze` and `capture` compile to nothing.

A host describes drained records with `PhyTrace` (an `oer_trace::EventSet`)
and decodes a snapshot slot with `PhySnapshot::decode`.
