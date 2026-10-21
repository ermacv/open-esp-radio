#![no_std]
use oer_memory::AffineSpscQueue;

pub fn endpoint() {
    let mut queue = AffineSpscQueue::<u8, 2>::new();
    let (mut sender, _receiver) = queue.split();
    let mut local = sender.borrow_local();
    #[cfg(ui_reject)]
    let _ = local.try_reserve(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = local.try_send(1);
}
