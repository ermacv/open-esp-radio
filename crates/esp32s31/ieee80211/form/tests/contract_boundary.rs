#![no_std]
use oer_radio_port::{Call, PortBackend};

// The typed FIFO is in Held<'id>; its affine sender-stage owner is in Local<'id>.
// Neither can be stored in the backend, which exists before 'id is introduced.
pub async fn enable_fifo<'a, 'id: 'a, B: PortBackend>(
    backend: &'a mut B,
    call: Call,
    held: &'a B::Held<'id>,
    local: &'a mut B::Local<'id>,
) {
    let _ = backend.enable(call, held, local).await;
}

pub fn setup_fifo<'a, 'id: 'a, B: PortBackend>(
    backend: &'a mut B,
    call: Call,
    held: &'a B::Held<'id>,
    local: &'a mut B::Local<'id>,
) -> B::Setup<'a, 'id> {
    backend.setup(call, held, local)
}
