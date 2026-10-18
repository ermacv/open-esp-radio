# IEEE 802.15.4 trace

`oer-i-trace` defines the trace points of the IEEE 802.15.4 MAC
driver as [`oer-base-trace`](../../../trace/src/lib.rs) events in
`Domain::I`. The target that emits them and the host that decodes
them link the same types, so no record is read as a bare number.

The types are chip-neutral and use the ESP-IDF driver's vocabulary: the
private driver states of `esp_i_dev.c`, `esp_i_tx_error_t`
and the receive and transmit abort reasons with their vendor values. Every
encoding uses integer operations only, so the MAC interrupt handler emits
them while the FPU is off.

| Kind | Channel | Event | Emitted at |
| --- | --- | --- | --- |
| `i.1` | 0 | `StateChange` | Every change of the engine's driver state |
| `i.2` | 1 | `TxOutcome` | The engine reports a transmission done or failed |
| `i.3` | 2 | `RxOutcome` | The engine delivers a frame or drops it into the stub buffer; the runtime drops one on a full event queue |
| `i.4` | 3 | `Abort` | The interrupt handler samples a receive or transmit abort, before it acts on the reason |
| `i.5` | 4 | `Interrupt` | Every engine interrupt entry: the state and the sampled events |
| `i.6` | 5 | `PowerSequence` | A chip HAL starts a transmission: the power-sequencing delays |
| `i.7` | 5 | `DcdcControl` | With `PowerSequence`: the DC-DC control and which power words have reserved bits set |
| `i.8` | 6 | `Timer` | The engine arms or stops a MAC timer, or handles its overflow |

The Domain::I range has twelve channels (mask bits 52 to 63);
7 to 11 are unassigned. The two power events share one channel because they
are emitted together. The engine emits behind its `trace` feature, the
ESP32-S31 HAL behind `i-trace` and the ESP32-S31 runtime behind
`trace`; each enables `oer-trace/record`. Without it, `oer_base_trace::emit`
compiles to nothing, and the HAL reads the power registers only with its
feature on and the channel enabled.

A host describes drained records with `ITrace` (an
`oer_base_trace::EventSet`).
