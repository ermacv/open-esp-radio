/// One addressing interface of a radio: a PAN ID, short and extended
/// address and frame-pending table of its own, as ESP-IDF's multi-PAN
/// driver keeps up to four (`esp_ieee802154_multipan_index_t`) for as many
/// OpenThread instances sharing one radio.
///
/// Every radio has the [`Interface::PRIMARY`] interface; a radio with
/// [`RadioCapabilities::MULTI_PAN`](crate::RadioCapabilities::MULTI_PAN)
/// has as many as its capabilities state
/// ([`Ieee802154Capabilities::interfaces`](crate::Ieee802154Capabilities::interfaces)).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Interface(u8);

impl Interface {
    /// The interface every radio has, which the configuration updates
    /// without an interface address.
    pub const PRIMARY: Self = Self(0);

    /// The interface with a zero-based `index`.
    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    /// The zero-based index.
    pub const fn index(self) -> u8 {
        self.0
    }
}
