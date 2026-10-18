use oer_esp32s31_ieee80211_mac::init::{
    StaLinkRxPolicyHardware, configure_sta_link_receive_policy,
};

use super::{StaJoinRadio, StaJoinStation, StaJoinStorage};

/// Complete production ESP32-S31 STA join port.
pub struct StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, C, O> {
    pub(super) radio: StaJoinRadio<'hardware, 'transmit, H, R, T, C>,
    pub(super) storage: StaJoinStorage<'scratch, O>,
    pub(super) station: StaJoinStation,
}

#[cfg_attr(test, allow(dead_code))]
impl<'hardware, 'transmit, 'scratch, H, R, T, C, O>
    StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, C, O>
{
    pub const fn new(
        radio: StaJoinRadio<'hardware, 'transmit, H, R, T, C>,
        storage: StaJoinStorage<'scratch, O>,
        station: StaJoinStation,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
        }
    }

    pub fn into_receive(self) -> R {
        self.radio.receive
    }

    /// Install the selected peer address into the pre-connected RX filter.
    pub fn prepare_authentication(&mut self)
    where
        H: StaLinkRxPolicyHardware,
    {
        configure_sta_link_receive_policy(self.radio.hardware, self.station.access_point.bssid);
    }
}
