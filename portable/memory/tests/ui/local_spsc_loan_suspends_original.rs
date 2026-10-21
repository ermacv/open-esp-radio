#![no_std]
use oer_memory::AffineSpscQueue;

pub fn endpoint() {
    let mut queue = AffineSpscQueue::<u8, 2>::new();
    let (mut sender, _receiver) = queue.split();
    let local = sender.borrow_local();
    let copy = local;
    #[cfg(ui_reject)]
    let _ = sender.try_reserve(); // ui-error: E0499
    let _ = copy.try_send(1);
    #[cfg(not(ui_reject))]
    let _ = sender.try_reserve();
}
