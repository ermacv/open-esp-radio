//! The modem ETM channels the public driver uses for timed operations, and
//! the baseband's recent RSSI.
//!
//! The pinned ESP-IDF driver (`esp_ieee802154_util.c`) programs channel zero
//! from a TIMER0 overflow to a transmit start and channel one from a TIMER1
//! overflow to a receive start. The Bluetooth parts of the matrix strobe only
//! their own bits of the channel-enable set and clear words, so these
//! operations take no lock. The engine composes the driver's channel-clear
//! and event/task sequences from them.
//!
//! SOURCE: ESP_IDF_7B9CC1AC_IEEE802154_DRIVER_ETM, ESP_IDF_7B9CC1AC_S31_SOC_ETM.

use oer_esp32s31_pac::modem_etm::{ch_event, ch_task};
use oer_espressif_ieee802154_engine::types::{Ieee802154EtmChannel, Ieee802154EtmRoute};
use oer_mmio::Placement;

use super::Ieee802154TaskOwner;

/// The event and the task a route connects.
///
/// SOURCE: ESP_IDF_7B9CC1AC_S31_IEEE802154_REG.
const fn route_ends(route: Ieee802154EtmRoute) -> (ch_event::Id, ch_task::Id) {
    match route {
        Ieee802154EtmRoute::Timer0ToTxStart => (
            ch_event::Id::Ieee802154Timer0Overflow,
            ch_task::Id::Ieee802154TxStart,
        ),
        Ieee802154EtmRoute::Timer0ToCcaTx => (
            ch_event::Id::Ieee802154Timer0Overflow,
            ch_task::Id::Ieee802154EdTrigTx,
        ),
        Ieee802154EtmRoute::Timer1ToRxStart => (
            ch_event::Id::Ieee802154Timer1Overflow,
            ch_task::Id::Ieee802154RxStart,
        ),
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154TaskOwner<P, E, B> {
    /// Read the channel-enable word: whether `channel` is enabled.
    pub(crate) fn etm_channel_enabled(&self, channel: Ieee802154EtmChannel) -> bool {
        let enabled = self.etm.read_channel_enable();
        match channel {
            Ieee802154EtmChannel::Channel0 => enabled.ch0(),
            Ieee802154EtmChannel::Channel1 => enabled.ch1(),
        }
    }

    /// Strobe `channel`'s bit of the write-trigger clear word. The vendor
    /// reads the word first; a trigger word reads as zero, so the written
    /// image is the same.
    pub(crate) fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        match channel {
            Ieee802154EtmChannel::Channel0 => self.etm.trigger_channel_enable_clear_ch0(),
            Ieee802154EtmChannel::Channel1 => self.etm.trigger_channel_enable_clear_ch1(),
        }
    }

    /// Strobe `channel`'s bit of the write-trigger set word.
    pub(crate) fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        match channel {
            Ieee802154EtmChannel::Channel0 => self.etm.trigger_channel_enable_set_ch0(),
            Ieee802154EtmChannel::Channel1 => self.etm.trigger_channel_enable_set_ch1(),
        }
    }

    /// Write the complete event word, then the complete task word, of the
    /// route's channel.
    pub(crate) fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        let (event, task) = route_ends(route);
        match route.channel() {
            Ieee802154EtmChannel::Channel0 => {
                self.etm.modify_ch0_event(|w| {
                    w.set_id(event.bits());
                });
                self.etm.modify_ch0_task(|w| {
                    w.set_id(task.bits());
                });
            }
            Ieee802154EtmChannel::Channel1 => {
                self.etm.modify_ch1_event(|w| {
                    w.set_id(event.bits());
                });
                self.etm.modify_ch1_task(|w| {
                    w.set_id(task.bits());
                });
            }
        }
    }

    /// The signed RSSI in dBm of the most recent baseband reception: the low
    /// byte of the shared baseband's current receive information, read live
    /// as `esp_ieee802154_get_recent_rssi` reads it. It reflects whichever
    /// protocol last received on the shared baseband; read side effects are
    /// unproven.
    ///
    /// SOURCE: BLOB_LIBBTBB_BT_BB_V2_ACCESSORS, ESP_IDF_7B9CC1AC_IEEE802154_RECENT_RSSI.
    pub(crate) fn recent_rssi(&self) -> i8 {
        self.rx_info.read_current_rx_info().rssi().cast_signed()
    }
}
