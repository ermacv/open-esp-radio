/// The affine CCMP-128 temporal-key owner shared by RSN and lower-MAC.
///
/// Installation moves this owner through the command FIFO into the backend.
/// Hardware programming and software CCMP borrow its bytes; there is no
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
    bytes: [u8; 16],
}

impl Ccmp128Key {
    /// Copy borrowed material directly into a zeroizing owner, without a
    /// by-value array argument or an unowned intermediate array. The source
    /// must itself have a zeroizing owner (PTK, GTK or a derivation buffer).
    pub fn new(bytes: &[u8; 16]) -> Self {
        let mut key = Self { bytes: [0; 16] };
        key.bytes.copy_from_slice(bytes);
        key
    }

    /// Lend material for hardware programming or software CCMP. The owner
    /// remains live while this view is borrowed.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.bytes
    }
}

impl core::fmt::Debug for Ccmp128Key {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Ccmp128Key(..)")
    }
}
