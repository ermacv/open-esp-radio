# IEEE 802.11 installation machine

`oer-protocols-ieee80211-core-installation`: one sans-IO machine a port,
which decides every transition of the port's installation for every role,
over the radio's life of the [port contract](../../contracts/port/README.md):
start, retune, halt and release, through the port's start, retune and stop,
the data plane's stop, the controller's tokens, the network's stop, the
units' return, the retirement and the release. It forbids unsafe code, has
no runtime panic path, and touches no port, no frame and no clock. There is
no restart and no reinstall: a port is installed once.

**A step yields one thing at a time and is resumed with its answer.**
`Machine::step(input, now)` returns a `Step` that borrows the machine. A
step that asks carries the right the call takes and a `Resume`, whose one
method takes the port contract's own answer (`StartPoll`, `RetunePoll`,
`StopPoll`, `DataStop`, `UnitsHome`, `Retired`) and returns the next step,
until the step rests (`Step::Rest`: a wake, with the instant of a
deadline, or the next round). No answer is an error: a start or a retune
that failed answers the radio stopped, and a stop always ends. While a step
is out the machine cannot be stepped (`tests/ui/second_step.rs`, E0499).
`drive` in `oer-protocols-ieee80211-runtime` carries a step out for every
role.

The machine is the one holder of the radio's rights (`Stopped`,
`Starting`, `Running`, `Retuning`, `Stopping`, `Retired`) and of the stop's
right, and of the data's rights while the data is stopped. A position holds
exactly the right its name says; a right a step carries out is the step's,
and the machine's position is then out until the answer gives the next one.
A stop lends the stop's right from the step (`Resume::right`); a pump has
no other way to it (`tests/ui/stop_without_step.rs`, E0599). A retirement
takes the data's rights and the units home; the release takes the stop's
right with the retirement, and after it no stop can be written.

A controller asks from its permit, which only the machine makes, typed by
the radio's position: `LiveStopped` runs the radio, `LiveRunning` retunes
it under the installation's closed gate, which the request lends
(`GateLent`) to the step where the port's retune begins; any permit halts
or releases. So a run of a running radio, a retune without the closed gate,
a second open request and a request after a release cannot be written; the
machine refuses nothing, and each request is closed by exactly one
`Ended`. What is waited for is said (`Step::Held`), never an end; only a
deadline ends a request that waits.

The port's one indication, "the port needs a stop" (`Input::NeedsStop`), is
answered by the stop the machine has: a start or a retune in progress is
given up and its request ends; a running radio's run is interrupted, the
controller is told, the data stops and then the radio.

The crate's test is the model's search over the real machine
(`src/tests.rs`): the model's environment, bounds and checks, with the
machine built from each state of the model over forged rights
(`life::forge`, the contract's `test-support` feature) and read back into
one. The table it finds equals `tests/model-table.txt`, the output of
`python3 -I design/installation-machine-model-next.py --table` of the
notes, line for line: 427 rows.
