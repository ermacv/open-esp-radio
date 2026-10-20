//! Bind protocol preparation to the backend-facing static handoff traits.
use crate::{
    EspNowEncryptedPeerId, EspNowLmk, EspNowPreparedEncryptedV1Tx, EspNowPreparedV1Tx,
    EspNowPreparedV2Tx,
};
use oer_ieee80211_port::espressif::*;

impl EspNowV1Tx for EspNowPreparedV1Tx<'_> {
    fn transmit_channel(self) -> oer_ieee80211_wire::channel::WifiChannel {
        self.transmit_channel()
    }
    fn station(self) -> oer_ieee80211_port::interface::BoundVirtualInterface {
        self.station()
    }
    fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode()
    }
    fn is_broadcast(self) -> bool {
        self.destination().is_broadcast()
    }
    fn encode(
        self,
        output: &mut [u8],
    ) -> Result<usize, oer_ieee80211_wire::extensions::espressif::esp_now::EspNowV1WireError> {
        self.encode(output)
    }
}
impl EspNowV2Tx for EspNowPreparedV2Tx<'_> {
    fn transmit_channel(self) -> oer_ieee80211_wire::channel::WifiChannel {
        self.transmit_channel()
    }
    fn station(self) -> oer_ieee80211_port::interface::BoundVirtualInterface {
        self.station()
    }
    fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode()
    }
    fn is_broadcast(self) -> bool {
        self.destination().is_broadcast()
    }
    fn encoded_len(self) -> usize {
        self.encoded_len()
    }
    fn encode(
        self,
        output: &mut [u8],
    ) -> Result<usize, oer_ieee80211_wire::extensions::espressif::esp_now::EspNowV2WireError> {
        self.encode(output)
    }
}
impl EspNowEncryptedTx for EspNowPreparedEncryptedV1Tx<'_> {
    fn home_channel(&self) -> oer_ieee80211_wire::channel::WifiChannel {
        (*self).home_channel()
    }
    fn station(&self) -> oer_ieee80211_port::interface::BoundVirtualInterface {
        (*self).station()
    }
}
impl EspNowKeyPeer for EspNowEncryptedPeerId {}
impl EspNowKeyMaterial for EspNowLmk {}
