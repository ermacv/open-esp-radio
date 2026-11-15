//! `drive`: the one carrier of the installation machine's steps, for every
//! role and every test.
//!
//! A step of the machine yields one thing at a time; `drive` answers each
//! ask on the port, or on the data plane, at once, passes each message and
//! end on to its party, and returns the step's [`Rest`]. It holds nothing:
//! a loop of one `match` with no field. It is the only caller of the
//! port's transitions in the repository, by type: six of the eight take a
//! proof and two the stop's right, which only the machine holds.
//!
//! Three answers of the port carry an owner that is not the machine's, and
//! `drive` splits them: a reinstall's `Installed` goes to the controller in
//! the one call that delivers the restart's end (`Parties::reinstalled`), a
//! release's device stays with the binding that holds the port, and an item
//! a cleanup hands back (`Unkeeping`) goes to the data plane. The machine gets what is left. A stop's answer is the
//! port's `Poll`, and the wait is the pump's: `drive` answers `Ready` with
//! the contract's `Result` and `Pending` with `pending`. That is the whole
//! of the mapping.

use core::task::{Context, Poll};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    DataPort, Input as CallInput, PortSink, SupportedChannel, Unkeeping, Unkept,
};
use oer_protocols_ieee80211_core_installation::{
    DataStop, Ended, Input, Machine, Rest, Step, Tell, Waiting,
};

/// The channel the machine of port `P` starts on.
pub type ChannelOf<'id, P> = SupportedChannel<<P as DataPort<'id>>::Caps>;

/// The installation machine of port `P`.
pub type MachineOf<'id, P> = Machine<'id, ChannelOf<'id, P>, <P as DataPort<'id>>::Fault>;

/// The parties a step speaks to beside the port: the data plane, the
/// controller and the network, as the composition binds them.
pub trait Parties<'id, P: DataPort<'id>> {
    /// The data plane: stop the data, answered at once.
    fn stop_data(&mut self) -> DataStop;
    /// The data plane: an item a cleanup could neither give the sink nor
    /// keep, to deliver.
    fn deliver(&mut self, unkept: Unkept<'id, P::Types>);
    /// A message to its party: the controller's tokens' end, the network's
    /// stop, or the data plane's flow.
    fn tell(&mut self, tell: Tell<'id>);
    /// The controller: the end of its restart, with the next installation's
    /// permissions, in one call.
    fn reinstalled(&mut self, ended: Ended<'id, P::Fault>, installed: P::Installed);
    /// The controller: the one end of its open request, or the release's.
    fn ended(&mut self, ended: Ended<'id, P::Fault>);
    /// The controller: the run was interrupted by a fault of the port.
    fn interrupted(&mut self);
    /// The controller: what the machine waits for changed.
    fn held(&mut self, waiting: &Waiting<P::Fault>);
}

/// The port of one installation and what its calls take beside it.
pub struct PortParts<'a, 'id, P: DataPort<'id>, S> {
    /// The port.
    pub port: &'a mut P,
    /// The installations a reinstall takes the next one from.
    pub installations: &'a mut P::Installations,
    /// The sink of the port's terminals and units.
    pub sink: &'a mut S,
}

/// Carry one step of `machine` out, from `input` at `now`, to its rest.
pub fn drive<'id, P: DataPort<'id>, S: PortSink<'id, P::Types>>(
    machine: &mut MachineOf<'id, P>,
    input: Input<'id, ChannelOf<'id, P>>,
    parts: &mut PortParts<'_, 'id, P, S>,
    parties: &mut impl Parties<'id, P>,
    cx: &mut Context<'_>,
    now: Instant,
) -> Rest {
    let PortParts {
        port,
        installations,
        sink,
    } = parts;
    let mut step = machine.step(input, now);
    loop {
        step = match step {
            Step::Start(proof, channel, resume) => {
                resume.answer(port.poll_start(cx, proof, channel, now))
            }
            // The wait is the pump's: a stop not confirmed yet is `pending`.
            Step::Stop(mut resume) => match port.poll_stop(resume.right(), cx, now) {
                Poll::Ready(answer) => resume.answer(answer),
                Poll::Pending => resume.pending(),
            },
            Step::StopData(resume) => resume.answer(parties.stop_data()),
            Step::TakeUnconsumed(stopped, resume) => {
                let answer = port.take_unconsumed(&stopped, *sink);
                resume.answer(stopped, answer)
            }
            Step::Clean(stopped, resume) => {
                let answer = match port.clean(&stopped, *sink) {
                    Ok(cleaned) => CallInput::Accepted(cleaned),
                    Err(Unkeeping { unkept, input }) => {
                        parties.deliver(unkept);
                        input
                    }
                };
                resume.answer(stopped, answer)
            }
            Step::TakeFreed(stopped, resume) => {
                let answer = port.take_unconsumed(&stopped, *sink);
                resume.answer(stopped, answer)
            }
            Step::Retire(stopped, resume) => resume.answer(port.retire(stopped)),
            Step::Reinstall(retired, resume) => match port.reinstall(retired, installations) {
                Ok((stopped, installed)) => {
                    let (ended, resume) = resume.installed(stopped);
                    parties.reinstalled(ended, installed);
                    resume.go()
                }
                Err(refused) => resume.refused(refused),
            },
            Step::Release(retired, right, releasing) => {
                // What the binding needs to give the radio up is the port's:
                // the binding holds the port and reaches it there.
                let answer = port.release(retired, right).map(|_released| ());
                releasing.answer(answer)
            }
            Step::Released(ended, rest) => {
                parties.ended(ended);
                return rest;
            }
            Step::Recover(mut resume) => {
                let answer = port.recover_for_stop(resume.right());
                resume.answer(answer)
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
