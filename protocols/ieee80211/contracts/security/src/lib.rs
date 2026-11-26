#![no_std]
#![forbid(unsafe_code)]

//! The IEEE 802.11 installed-protection contract.
//!
//! The secret material a protocol installs and a backend programs has one
//! owner here, apart from the frame codecs: they carry field syntax, never a
//! key. Port, backends, RSN and the software cipher borrow it; none of them
//! has to depend on another to share it.

/// The affine CCMP-128 temporal-key owner shared by RSN and lower-MAC.
///
/// Installation moves this owner through the command FIFO into the backend.
/// Hardware programming borrows its bytes and software CCMP its AES-128
/// key (`oer_portable_crypto::Aes128Key`, held inside); there is no
/// cloning or owned byte-extraction API, and `Debug` redacts the material.
/// The byte owner needs no C layout or word alignment: backends encode the
/// borrowed octets into register words instead of casting this object.
///
/// Normal `Drop` zeroizes its storage. In quarantine, `Drop` is not called
/// and the key remains in memory until the chip is reset; this retention is
/// intentional. Zeroization covers the owner's storage, not copies the
/// compiler may make while moving it.
#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[must_use]
pub struct Ccmp128Key {
    key: oer_portable_crypto::Aes128Key,
}

impl Ccmp128Key {
    /// Copy borrowed material directly into a zeroizing owner, without a
    /// by-value array argument or an unowned intermediate array. The source
    /// must itself have a zeroizing owner (PTK, GTK or a derivation buffer).
    pub const fn new(bytes: &[u8; 16]) -> Self {
        Self {
            key: oer_portable_crypto::Aes128Key::copy_from(bytes),
        }
    }

    /// Lend material for hardware programming or software CCMP. The owner
    /// remains live while this view is borrowed.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.key.as_bytes()
    }

    /// Lend the key to an AES-128 engine for software CCMP, without a copy
    /// a frame.
    pub const fn aes_key(&self) -> &oer_portable_crypto::Aes128Key {
        &self.key
    }
}

impl core::fmt::Debug for Ccmp128Key {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Ccmp128Key(..)")
    }
}
