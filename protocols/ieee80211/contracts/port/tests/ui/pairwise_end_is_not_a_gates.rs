#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    CommandPoll, DataPort, GateClosed, InCommand, PairwiseEnd, PairwiseHeld, PortSink, Station,
};
// A pairwise key command's poll ends with its own kind's end: the slot,
// installed or empty. It cannot be matched as the gate's closing.
pub fn poll<'a, P: DataPort<'a>>(
    port: &mut P,
    command: InCommand<'a, PairwiseHeld<'a, Station>>,
    now: Instant,
    sink: &mut impl PortSink,
) {
    match port.poll_pairwise(command, now, sink) {
        CommandPoll::Ended(_, PairwiseEnd::Installed(_) | PairwiseEnd::Empty(_)) => {}
        CommandPoll::Pending(_) => {}
    }
    #[cfg(ui_reject)]
    let _: CommandPoll<'a, PairwiseHeld<'a, Station>, GateClosed<'a>> = port.poll_pairwise(command, now, sink); // ui-error: E0308
    let _ = core::marker::PhantomData::<GateClosed<'a>>;
}
