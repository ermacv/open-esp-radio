# IEEE 802.11 installation machine

`oer-protocols-ieee80211-core-installation`: one sans-IO machine a port,
which decides every transition of the port's installation for every role:
start, retune, halt, restart and release, through the port's stop, the
data plane's, the controller's tokens, the drain of what a sink refused,
the network's stop, the retirement, the reinstall and the release. It
forbids unsafe code, has no runtime panic path and neither waits nor
performs I/O: a step (`Machine::step`) takes one `Event` and writes the
asks and the `Report`s into the `Outbox` its pump lends.

The machine is the one holder of the port's proofs ([port
contract](../../contracts/port/README.md), the installation's
transitions). A phase that holds one holds it by type: stopped and
settling the stop's `Stopped`, retired the `Retired`; an ask that takes a
proof takes it from the machine, so the machine cannot hold two, ask
without one, or keep one it gave.

Its rules: a request is answered once (refused at once, or closed by one
final report); endings are ordered (halt, restart, release with the
network staying, release with it stopping), a stronger one supersedes a
weaker one; a stop ends a start; halted means the data is home; a
retirement is asked only with the data stopped, the tokens ended and the
unconsumed items taken, and asked again when what it named came home; a
release that stops the network waits for its stop; a fault is reported in
every phase with the port's answer, and while starting or running ends
the run by a stop; `held` is a report, never an end; a new installation
ends in turn; a proof the port did not take is followed by a stop; a stop
answered "released" ends the machine.

The machine is its design's executable model, transition for
transition. The crate's tests walk the model's search over it (every
answer to every ask, every request in every state, a fault in every
phase, the model's bounds) checking its five properties, and compare the
machine's table of transitions with the model's, committed as
`tests/model-table.txt` (`python3 -I design/installation-machine-model.py
--table | grep '^|'` of the notes refreshes it).
