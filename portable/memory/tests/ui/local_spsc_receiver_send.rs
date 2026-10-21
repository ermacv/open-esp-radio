#![no_std]
use oer_memory::AffineSpscQueue;

fn send<T: Send>(_: T) {}
pub fn endpoint() {
    let mut queue = AffineSpscQueue::<u8, 2>::new();
    let (_sender, mut receiver) = queue.split();
    #[cfg(ui_reject)]
    send(receiver.borrow_local()); // ui-error: E0277
    #[cfg(not(ui_reject))]
    send(receiver);
}
