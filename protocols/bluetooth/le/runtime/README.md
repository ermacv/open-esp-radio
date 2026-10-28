# Bluetooth LE runtime

`oer-bluetooth-le-runtime` owns executor-independent DTM waits and the
one-credit HCI service loop. It depends on the portable controller, LE port
and HCI boundary, without a backend, HAL, executor or task spawner. This
crate separates waiting from the controller's synchronous protocol state.

`dtm::submit` borrows the controller's seat while awaiting capacity. It then
builds the core's owned request, submits it synchronously, applies the
receipt/refusal and releases unused capacity without another suspension.
Cancellation before admission leaves the command, seat and lease in the
core. After admission, calling `submit` again returns the outstanding
terminal obligation without submitting twice. `dtm::complete` checks that
obligation before awaiting the port event and applying the terminal value;
cancelling that wait preserves both the command and backend custody.

`host::serve` takes a `Transport` and a `ControllerPort`. The transport owns
framing, command storage, entropy preparation and packet delivery. The port
lends one phase at a time and exposes the independent physical clock latch.
The order is receive, optional Rand latch/entropy, accept and consume,
submit, physical latch, terminal, send and release command credit. A blocked
or failed send retains the immutable response in the controller.

The firmware composition owns USB/H4, HAL entropy, installation,
disable/uninstall, fail-stop/reset policy and task declarations. Reset
returns the controller's affine token to that composition; acknowledgement
requires a fresh joined installation of the same slot/path. The composition
supplies that response before the next command and supplies its stop opcode.
No runtime future spawns another task. Any composition timer uses
`oer-time`; runtime port waits and signals remain portable.
