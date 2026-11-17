#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    CommandPoll, DataPort, GateClosed, InCommand, PairwiseCommand, PairwiseEnd, PortSink,
};
// A pairwise key command's poll ends with its own kind's end: the slot,
// installed or empty. It cannot be matched as the gate's closing.
pub fn poll<'a, P: DataPort<'a>>(
    port: &mut P,
    command: InCommand<'a, PairwiseCommand<'a>>,
    now: Instant,
    sink: &mut impl PortSink<'a, P::Ack, P::Denial>,
) {
    match port.poll_pairwise(command, now, sink) {
        CommandPoll::Ended(_, PairwiseEnd::Installed(_) | PairwiseEnd::Empty(_)) => {}
        CommandPoll::Pending(_) => {}
    }
    #[cfg(ui_reject)]
    let _: CommandPoll<'a, PairwiseCommand<'a>, GateClosed<'a>> = port.poll_pairwise(command, now, sink); // ui-error: E0308
    let _ = core::marker::PhantomData::<GateClosed<'a>>;
}
