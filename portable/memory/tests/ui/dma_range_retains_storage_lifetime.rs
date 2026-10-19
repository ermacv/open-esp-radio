#![no_std]
use oer_memory::StableDmaRange;

pub fn bind<'storage>(storage: &'storage mut [u8; 16]) -> StableDmaRange<'storage> {
    // SAFETY: this host fixture maps the complete allocation to a synthetic
    // address. No DMA actor exists, and the range retains the storage lifetime.
    let (_, range) = unsafe { StableDmaRange::from_owner(storage, 100) }
        .expect("host allocation fits");
    #[cfg(ui_reject)]
    let range: StableDmaRange<'static> = range; // ui-error: lifetime may not live long enough
    range
}
