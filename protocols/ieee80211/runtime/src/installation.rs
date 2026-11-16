//! `drive`: the one carrier of the installation machine's steps, for every
//! role and every test.
//!
//! A step of the machine yields one thing at a time; `drive` answers each
//! ask on the port, or on the data plane, at once, passes each message and
//! end on to its party, and returns the step's [`Rest`]. It holds nothing:
//! a loop of one `match` with no field. It is the only caller of the
//! port's life transitions in the repository, by type: each takes a right
//! only the machine holds, and the stop takes the stop's right the step
//! lends.
//!
//! A begun start, retune or stop is polled in the same turn: the port's
//! answer to the poll is the machine's. A release's hardware stays with the
//! binding that holds the port.

use core::task::{Context, Poll};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::RadioPort;
use oer_protocols_ieee80211_core_installation::{
    DataStop, Ended, Input, Machine, Rest, RetuneAsk, StartAsk, Step, StopAsk, Tell, Waited,
};

/// The installation machine of port `P`.
pub type MachineOf<'id, P> =
    Machine<'id, <P as RadioPort<'id>>::Channel, <P as RadioPort<'id>>::Data>;

/// The parties a step speaks to beside the port: the data plane, the
/// controller and the network, as the composition binds them.
pub trait Parties<'id, P: RadioPort<'id>> {
    /// The data plane: stop the data, answered at once.
    fn stop_data(&mut self) -> DataStop<P::Data>;
    /// A message to its party: the controller's tokens' end, the network's
    /// stop, or the data plane's flow with its rights.
    fn tell(&mut self, tell: Tell<'id, P::Data>);
    /// The controller: the one end of its open request, or the release's.
    fn ended(&mut self, ended: Ended<'id>);
    /// The controller: the run was interrupted, since the port needs a stop.
    fn interrupted(&mut self);
    /// The controller: what the machine waits for changed.
    fn held(&mut self, waiting: Waited);
}

/// Carry one step of `machine` out, from `input` at `now`, to its rest.
pub fn drive<'id, P: RadioPort<'id>>(
    machine: &mut MachineOf<'id, P>,
    input: Input<'_, 'id, P::Channel>,
    port: &mut P,
    parties: &mut impl Parties<'id, P>,
    cx: &mut Context<'_>,
    now: Instant,
) -> Rest {
    let mut step = machine.step(input, now);
    loop {
        step = match step {
            Step::Start(ask, resume) => {
                let starting = match ask {
                    StartAsk::Begin(stopped, channel) => port.start(stopped, channel, now),
                    StartAsk::Going(starting) => starting,
                };
                resume.answer(port.poll_start(starting, cx, now))
            }
            Step::Retune(ask, resume) => {
                let retuning = match ask {
                    RetuneAsk::Begin(running, channel, gate) => {
                        port.retune(running, channel, gate, now)
                    }
                    RetuneAsk::Going(retuning) => retuning,
                };
                resume.answer(port.poll_retune(retuning, cx, now))
            }
            Step::Stop(ask, mut resume) => {
                let stopping = match ask {
                    StopAsk::Running(running) => port.stop(resume.right(), running),
                    StopAsk::Starting(starting) => port.stop(resume.right(), starting),
                    StopAsk::Retuning(retuning) => port.stop(resume.right(), retuning),
                    StopAsk::Going(stopping) => stopping,
                };
                resume.answer(port.poll_stop(stopping, cx, now))
            }
            Step::StopData(resume) => resume.answer(parties.stop_data()),
            Step::UnitsHome(resume) => match port.poll_units_home(cx) {
                Poll::Ready(units) => resume.home(units),
                Poll::Pending => resume.pending(),
            },
            Step::Retire(stopped, data, units, resume) => {
                resume.answer(port.retire(stopped, data, units))
            }
            Step::Release(retired, right, releasing) => {
                // What the binding needs to give the radio up is the port's:
                // the binding holds the port and reaches it there.
                let _released = port.release(retired, right);
                releasing.answer()
            }
            Step::Released(ended, rest) => {
                parties.ended(ended);
                return rest;
            }
            Step::Tell(tell, resume) => {
                parties.tell(tell);
                resume.go()
            }
            Step::Ended(ended, resume) => {
                parties.ended(ended);
                resume.go()
            }
            Step::Interrupted(resume) => {
                parties.interrupted();
                resume.go()
            }
            Step::Held(resume) => {
                parties.held(resume.waiting());
                resume.go()
            }
            Step::Rest(rest) => return rest,
        };
    }
}
