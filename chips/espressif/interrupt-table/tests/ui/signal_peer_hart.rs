use oer_espressif_interrupt_table::signal::{
    FromPeerHart, IrqSignal,
    host::{Deferred, Foreign},
};
oer_espressif_interrupt_table::__fake_parts!();

pub fn on_radio(_: &mut RadioIrq) {}
pub fn on_wake(_: &mut AbsentIrq) {}
macro_rules! table {
    ($hart:literal) => {
        oer_espressif_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { app_wake: 1; receiver: 2; }
            executor: ExecutorToken = RadioIrq => on_radio, line 17, context receiver, hart 0, Executor;
            peer: PeerWakeToken = AbsentIrq => on_wake, line 16, context app_wake, hart $hart, Hardware;
        }
    };
}
// The raising executor's wake entry sits on the receiver's hart.
#[cfg(ui_reject)]
table!(0);
#[cfg(not(ui_reject))]
table!(1);

const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromPeerHart<Foreign, PeerWakeToken>, ExecutorToken>(); // ui-error@outside: E0080: evaluation panicked: a peer-hart raiser must run on another hart than its executor
