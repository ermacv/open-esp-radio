#![no_std]
use oer_radio_port::{Ended, FailStopped, PortBackend};

pub fn ended<'id, B: PortBackend>(
    proof: FailStopped<'id, B>,
    outcome: oer_radio_port::Abandon,
) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let proof = FailStopped {
        outcome, // ui-error: E0451
        brand: core::marker::PhantomData,
        backend: core::marker::PhantomData,
    };
    Ended::fail_stopped(proof, ())
}
