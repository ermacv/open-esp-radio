#[path = "__HART_BINDING__"]
mod hart_binding;

oer_espressif_interrupt_table::__fake_parts!();
pub fn on_wake(_: &mut RadioIrq) {}
pub fn on_peer(_: &mut AbsentIrq) {}

macro_rules! table {
    ($hart:literal) => {
        oer_espressif_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { executor: 1; peer_executor: 2; }
            wake: Wake = RadioIrq => on_wake, line 17, context executor, hart $hart, Executor;
            // An executor on the non-boot hart, whose startup names hart 1.
            peer: Peer = AbsentIrq => on_peer, line 18, context peer_executor, hart 1, Executor;
        }
    };
}
#[cfg(not(ui_reject))]
table!(0);
#[cfg(ui_reject)]
table!(1);

pub fn startup() {
    hart_binding::check::<0, Wake>();
}

pub fn startup_on_hart_1() {
    hart_binding::check::<1, Peer>();
}
