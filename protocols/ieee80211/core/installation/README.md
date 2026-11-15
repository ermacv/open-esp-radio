# IEEE 802.11 installation machine

`oer-protocols-ieee80211-core-installation`: one sans-IO machine a port,
which decides every transition of the port's installation for every role:
start, retune, halt, restart and release, through the port's stop, the data
plane's, the controller's tokens, the drain of what a sink refused, the
network's stop, the retirement, the reinstall and the release. It forbids
unsafe code, has no runtime panic path, and touches no port, no frame and
no clock. Its design is `design/installation-machine-2026-11-14.md` of the
notes, and its executable model `design/installation-machine-model-b3a.py`.

**A step yields one thing at a time and is resumed with its answer.**
`Machine::step(input, now)` returns a `Step` that borrows the machine. A
step that asks carries a `Resume`, whose one method takes the port
contract's own answer and returns the next step, until the step rests
(`Step::Rest`, which names what comes next: a wake, with the instant of a
deadline or of the retry time, or the next round). Nothing is kept between
an ask and its answer, and while a step is out the machine cannot be
stepped (`tests/ui/second_step.rs`, E0499). `drive` in
`oer-protocols-ieee80211-runtime` carries a step out for every role.

The machine is the one holder of the port's proofs and of the stop's
right. A position holds exactly the proof its name says: stopped the
stop's, starting the start's, retired the retirement's. A proof an ask
takes or borrows travels in the `Step`, and the machine's position is then
"stopping", which holds none, so a step that is dropped leaves a state a
stop mends. Whether the port holds an installation is asked once a step,
at its beginning: a released machine, and one whose release is asked, rest
on every input; every other function, and every resume but the release's,
works on a view of the port that holds an installation (the right, the
position, the goal and what is waited for). A stop and a recovery lend the
right from that view (`Resume::right`); the stop's answer is
`Resume::answer` with the contract's `Result`, or `Resume::pending`; a pump
has no other way to the right (`tests/ui/stop_without_step.rs`). A
retirement granted ends its step with a round, and the retired
installation goes on from the next step's beginning, where the machine is
whole: a release takes the right with the retirement's proof, and after it
no stop can be written.

A controller asks from its permit (`Live`: anything; `Spent`: a restart or
a release), which only the machine makes: one at its placement, one at each
end but a release's. So a second open request, a run of an installation
without tokens and a request after a release cannot be written; the
machine refuses nothing, and each request is closed by exactly one
`Ended`. An end is `Result<End, Unreached>`; what is waited for is a
status (`Step::Held`, `Machine::waiting`), never an end; only a deadline
ends a request that waits.

Three answers of the port carry an owner that is not the machine's, and
`drive` splits them before the machine hears the answer: a reinstall's
`Installed`, a release's device, and an item a cleanup hands back.

The crate's test is the model's search over the real machine
(`src/tests.rs`): the model's environment, bounds and eight checks, with
the machine built from each state of the model and read back into one. The
table it finds equals `tests/model-table.txt`, the output of `python3 -I
design/installation-machine-model-b3a.py --table`, line for line; the same
search holds at the model's wider bounds.
