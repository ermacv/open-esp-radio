#![no_std]
use core::ops::ControlFlow;
use oer_radio_port::{Enabled, Owner, PortBackend, SlotIdentity};

pub fn disable<'p, 'id, Id: SlotIdentity, B: PortBackend<Id>, S>(
    owner: Owner<'p, 'id, Id, B, Enabled>,
    sink: &mut S,
) where
    S: FnMut(B::Terminal<'id>) -> ControlFlow<B::Terminal<'id>>,
{
    let disabling = owner.disable(sink);
    #[cfg(ui_reject)]
    let _second = &mut *sink; // ui-error: E0499
    drop(disabling);
    #[cfg(not(ui_reject))]
    let _second = &mut *sink;
}
