#![no_std]
use oer_radio_port::PublishedPoison;
pub fn finish(published: PublishedPoison) -> PublishedPoison {
    #[cfg(ui_reject)]
    let _copy = published.clone(); // ui-error: E0599
    published
}
