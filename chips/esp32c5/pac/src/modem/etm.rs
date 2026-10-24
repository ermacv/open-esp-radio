//! Capability over the modem event-task matrix channels IEEE 802.15.4 uses.
//!
//! The public IEEE 802.15.4 driver programs channels zero and one. No other
//! channel user is modelled on the ESP32-C5, so the channel owner is the
//! whole reviewed matrix view. The set and clear words are not reviewed as
//! write-trigger words: each transaction reads the word and writes it back
//! with its channel bit added, as the vendor driver does.

use crate::generated::{modem_etm::ModemEtm, peripheral_ownership::ModemEtmPeripherals};

/// ETM channels zero and one, owned by IEEE 802.15.4.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct Ieee802154EtmChannels {
    pub(crate) registers: ModemEtm,
}

/// Bind the unique modem ETM owner to the IEEE 802.15.4 channel capability.
pub(crate) fn ieee802154_channels(peripherals: ModemEtmPeripherals) -> Ieee802154EtmChannels {
    let ModemEtmPeripherals { modem_etm } = peripherals;
    Ieee802154EtmChannels {
        registers: modem_etm,
    }
}
