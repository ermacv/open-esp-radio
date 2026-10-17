#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::mem::MaybeUninit;
use oer_ieee80211_mac::ccmp::Ccmp128Key;
use zeroize::{ZeroizeOnDrop, Zeroizing};

#[test]
#[allow(
    unsafe_code,
    reason = "observe initialized key bytes after its real destructor, while the backing storage remains live"
)]
fn ordinary_drop_erases_key_storage() {
    fn zeroizes_on_drop<T: ZeroizeOnDrop>(_: &T) {}

    let source = Zeroizing::new([0x9d; 16]);
    let mut storage = MaybeUninit::new(Ccmp128Key::new(&source));
    let key = storage.as_mut_ptr();
    // SAFETY: storage contains an initialized key. This borrow ends before
    // drop_in_place; only a byte offset is retained, without assuming layout.
    let offset = unsafe {
        let initialized = &*key;
        zeroizes_on_drop(initialized);
        assert_eq!(initialized.as_bytes(), &*source);
        assert_eq!(format!("{initialized:?}"), "Ccmp128Key(..)");
        initialized
            .as_bytes()
            .as_ptr()
            .offset_from_unsigned(key.cast())
    };
    // SAFETY: key points to the initialized, exclusively owned value. Run its
    // normal destructor exactly once; MaybeUninit will not drop it again.
    unsafe { core::ptr::drop_in_place(key) };
    // SAFETY: storage is still allocated. The destructor writes all sixteen
    // key bytes; inspect only those initialized u8 values, without borrowing
    // the destroyed Ccmp128Key or reading any possible padding.
    let erased = unsafe { core::slice::from_raw_parts(key.cast::<u8>().add(offset), 16) };
    assert_eq!(erased, &[0; 16]);
    assert_eq!(&*source, &[0x9d; 16]);
}
