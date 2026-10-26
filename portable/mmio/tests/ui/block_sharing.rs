#![no_std]
use oer_mmio::{Block, Fixed, Part, Reader, Registers, Words, sharing};

pub struct Owned;
impl Registers for Owned {
    type Sharing = sharing::Owned;
    const WORDS: &'static [usize] = &[0];
}

pub struct Shared;
impl Registers for Shared {
    type Sharing = sharing::Shared;
    const WORDS: &'static [usize] = &[0];
}

impl Words for Owned {
    const OWNED: &'static [usize] = &[0];
    const OBSERVED: &'static [usize] = &[];
}

fn sync<T: Sync>() {}
fn send<T: Send>() {}

pub fn sharing() {
    sync::<Block<Fixed<0x1000>, Shared>>();
    sync::<Reader<Fixed<0x1000>, Owned, Owned>>();
    send::<Block<Fixed<0x1000>, Owned>>();
    send::<Part<Fixed<0x1000>, Owned, Owned>>();

    #[cfg(ui_reject)]
    {
        sync::<Block<Fixed<0x1000>, Owned>>(); // ui-error: E0277
        sync::<Part<Fixed<0x1000>, Owned, Owned>>(); // ui-error: E0277
        sync::<Block<oer_mmio::Host<'static>, Shared>>(); // ui-error: E0277
    }
}
