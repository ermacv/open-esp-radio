#![no_std]
use oer_memory::PinnedDmaTxPoolHandle;

type Handle = PinnedDmaTxPoolHandle<32, 8, 4, 1>;

pub fn transfer(handle: Handle) -> Handle {
    #[cfg(ui_reject)]
    let _first = handle;
    handle // ui-error: E0382
}
