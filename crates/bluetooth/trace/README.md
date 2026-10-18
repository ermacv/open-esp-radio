# Bluetooth LE trace

`oer-bluetooth-trace` defines the trace points of the Bluetooth LE
Controller core, the ESP32-S31 radio role and its runtime as
[`oer-trace`](../../../trace/src/lib.rs) events in `Domain::Bluetooth`. The
target that emits them and the host that decodes them link the same types,
so no record is read as a bare number. Event identities are the Controller's
`EventId` values, and instants are the low word of the radio time in
microseconds.

| Kind | Channel | Event | Emitted at |
| --- | --- | --- | --- |
| `bluetooth.1` | 0 | `RoleRequest` | The S31 radio role answers a portable request: its kind, the answer and the lead from the radio time to the first air window |
| `bluetooth.2` | 1 | `RoleEventEnd` | The role ends an event: its result and the PDUs it reported and discarded |
| `bluetooth.3` | 2 | `RolePdu` | The role reports a PDU of an advertising or scan event |
| `bluetooth.4` | 3 | `AdvertiserStep` | The Controller's advertising set is configured, plans, skips or ends an event, or leaves the radio |
| `bluetooth.5` | 4 | `ConnectionIndication` | The Controller judges a PDU received by a connectable advertising event as a connection indication; another PDU type, such as a `SCAN_REQ`, is `OtherPdu`, apart from a `CONNECT_IND` that fails validation |
| `bluetooth.6` | 5 | `PeripheralOpened` | The Controller opens the peripheral connection: the first anchor and the interval |
| `bluetooth.7` | 6 | `PeripheralEvent` | A connection event is requested, admitted or refused, and ends with or without peer activity |
| `bluetooth.8` | 7 | `PeripheralClosed` | The connection closes, or the radio refuses its opening, with the HCI reason the Host gets |
| `bluetooth.9` | 8 | `QueueLoss` | The S31 runtime's outcome queue drops a report, and the consumer takes the loss |
| `bluetooth.10` | 9 | `Lifecycle` | The S31 runtime starts a transition of its port's owner (`Enable`, `Disable`) and ends it: done, poisoned or past its bound; the two records time it |

The `Domain::Bluetooth` range has twelve channels (mask bits 40 to 51);
10 and 11 are unassigned. The per-PDU channel 2 has the highest rate; a
session that follows the connection's steps can leave it off.

`oer-bluetooth-controller`, `oer-esp32s31-bluetooth-radio` and
`oer-esp32s31-bluetooth-runtime` emit behind their own `trace` feature, each
of which enables `oer-trace/record`; without it nothing is built, and the
recording never depends on another crate's features. Their host tests record
every event on the calling thread. The HIL agent's `bluetooth-trace` feature
turns all three on.

A host describes drained records with `BluetoothTrace` (an
`oer_trace::EventSet`).
