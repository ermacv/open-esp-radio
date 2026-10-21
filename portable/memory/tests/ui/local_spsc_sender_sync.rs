#![no_std]
use oer_memory::AffineSpscQueue;

fn sync<T: Sync>(_: &T) {}
pub fn endpoint() {
    let mut queue = AffineSpscQueue::<u8, 2>::new();
    let (mut sender, _receiver) = queue.split();
    #[cfg(ui_reject)]
    sync(&sender.borrow_local()); // ui-error: E0277
    #[cfg(not(ui_reject))]
    sync(&queue);
}
