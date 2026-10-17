//! Modem event-task matrix transactions used by the public IEEE 802.15.4
//! driver for timed operations.
//!
//! The pinned ESP-IDF driver (`esp_ieee802154_util.c`) programs channel zero
//! from a TIMER0 overflow to a transmit start and channel one from a TIMER1
//! overflow to a receive start. The Bluetooth runtime owns channels four
//! through seven of the same matrix, so this surface holds only the
//! [`Ieee802154EtmChannels`](crate::modem::etm::Ieee802154EtmChannels)
//! capability and names the two IEEE 802.15.4 channels and the three routes
//! the driver programs. Each method is one register transaction; the HAL
//! composes the driver's channel-clear and event/task sequences from them.

use super::Ieee802154RegisterLease;

/// Modem ETM channel owned by the IEEE 802.15.4 driver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmChannel {
    /// `IEEE802154_ETM_CHANNEL0`, used for timed transmit.
    Channel0,
    /// `IEEE802154_ETM_CHANNEL1`, used for timed receive.
    Channel1,
}

/// Event-to-task route the public driver programs on its ETM channels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmRoute {
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_TX_START`.
    Timer0ToTxStart,
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_ED_TRIG_TX`,
    /// a clear-channel assessment followed by transmit.
    Timer0ToCcaTx,
    /// Channel one: `ETM_EVENT_TIMER1_OVERFLOW` to `ETM_TASK_RX_START`.
    Timer1ToRxStart,
}

impl Ieee802154EtmRoute {
    /// The channel the public driver programs for this route.
    pub const fn channel(self) -> Ieee802154EtmChannel {
        match self {
            Self::Timer0ToTxStart | Self::Timer0ToCcaTx => Ieee802154EtmChannel::Channel0,
            Self::Timer1ToRxStart => Ieee802154EtmChannel::Channel1,
        }
    }
}

impl Ieee802154RegisterLease<'_> {
    /// Read the channel-enable word and report whether `channel` is enabled.
    pub fn etm_channel_enabled(&self, channel: Ieee802154EtmChannel) -> bool {
        let status = self.etm.registers.channel_enable().read();
        match channel {
            Ieee802154EtmChannel::Channel0 => status.ch0().bit(),
            Ieee802154EtmChannel::Channel1 => status.ch1().bit(),
        }
    }

    /// Write only `channel`'s bit to the write-trigger clear word. The
    /// vendor reads the word first; a trigger word reads as zero, so the
    /// written image is the same.
    pub fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        let etm = &self.etm.registers;
        match channel {
            Ieee802154EtmChannel::Channel0 => {
                crate::svd::fixed_register_image::disable_ieee802154_etm_channel0(etm);
            }
            Ieee802154EtmChannel::Channel1 => {
                crate::svd::fixed_register_image::disable_ieee802154_etm_channel1(etm);
            }
        }
    }

    /// Write only `channel`'s bit to the write-trigger set word.
    pub fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        let etm = &self.etm.registers;
        match channel {
            Ieee802154EtmChannel::Channel0 => {
                crate::svd::fixed_register_image::enable_ieee802154_etm_channel0(etm);
            }
            Ieee802154EtmChannel::Channel1 => {
                crate::svd::fixed_register_image::enable_ieee802154_etm_channel1(etm);
            }
        }
    }

    /// Write the complete event word, then the complete task word, of the
    /// route's channel.
    pub fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        let etm = &self.etm.registers;
        match route {
            Ieee802154EtmRoute::Timer0ToTxStart => {
                crate::svd::fixed_register_sequence::route_ieee802154_etm_timer0_to_tx_start(etm);
            }
            Ieee802154EtmRoute::Timer0ToCcaTx => {
                crate::svd::fixed_register_sequence::route_ieee802154_etm_timer0_to_ed_trig_tx(etm);
            }
            Ieee802154EtmRoute::Timer1ToRxStart => {
                crate::svd::fixed_register_sequence::route_ieee802154_etm_timer1_to_rx_start(etm);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Ieee802154EtmChannel, Ieee802154EtmRoute};

    /// Timed transmit programs channel zero and timed receive channel one
    /// (`esp_ieee802154_dev.c` `ieee802154_transmit_at` and
    /// `ieee802154_receive_at`).
    #[test]
    fn timed_transmit_and_receive_use_disjoint_channels() {
        assert_eq!(
            Ieee802154EtmRoute::Timer0ToTxStart.channel(),
            Ieee802154EtmChannel::Channel0
        );
        assert_eq!(
            Ieee802154EtmRoute::Timer0ToCcaTx.channel(),
            Ieee802154EtmChannel::Channel0
        );
        assert_eq!(
            Ieee802154EtmRoute::Timer1ToRxStart.channel(),
            Ieee802154EtmChannel::Channel1
        );
    }
}
