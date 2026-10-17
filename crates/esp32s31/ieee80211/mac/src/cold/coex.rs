//! Ownership boundary for the complete cold COEX/PTI transaction.

use oer_esp32s31_hal::{coex::CoexEventId, ieee80211::mac::WifiMacColdHal};

/// The four OSI coexistence event numbers queried by complete cold init.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MacCoexEvent {
    Event1 = 1,
    Event3 = 3,
    Event10 = 10,
    Event15 = 15,
}

impl MacCoexEvent {
    /// The shared coexistence event this MAC query reads.
    ///
    /// Its priority comes from the radio arbiter's table, never from a Wi-Fi
    /// constant. The cold values are still required in a Wi-Fi-only build.
    /// Complete `libpp.a[hal_mac.o]::hal_init` passes event three to
    /// `hal_set_rx_ack_pti`, which directly programs the MAC's immediate
    /// RX-ACK scheduler priority. It is not merely a request to a running
    /// Bluetooth coexistence task: keeping RX-ACK at zero lets a pending
    /// ordinary EDCA queue outrank the response transaction.
    ///
    /// SOURCE: complete `libpp.a[hal_mac.o,hal_coex.o]::{
    /// hal_init,hal_set_rx_ack_pti,hal_set_wifi_default_pti,
    /// hal_set_ofdma_sequence_pti}`.
    pub const fn coex_event(self) -> CoexEventId {
        match CoexEventId::new(self as u8) {
            Some(event) => event,
            None => unreachable!(),
        }
    }
}

/// One byte returned by `_coex_pti_get`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacCoexPti(u8);

impl MacCoexPti {
    pub const fn from_osi_value(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// All callback samples consumed by the complete cold COEX setter graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdCoexPti {
    rx_ack: MacCoexPti,
    wifi_default: MacCoexPti,
    tb: [MacCoexPti; 7],
    beamforming: [MacCoexPti; 3],
    multi_target: [MacCoexPti; 2],
}

pub trait MacCoexPtiSource {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti;
}

impl MacColdCoexPti {
    /// Query the platform in the exact callback order of complete `hal_init`
    /// and `hal_set_ofdma_sequence_pti`.
    pub fn query<S: MacCoexPtiSource>(source: &mut S) -> Self {
        let rx_ack = source.mac_coex_pti(MacCoexEvent::Event3);
        let wifi_default = source.mac_coex_pti(MacCoexEvent::Event15);
        let tb = [
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event1),
        ];
        let beamforming = [
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
        ];
        let multi_target = [
            source.mac_coex_pti(MacCoexEvent::Event10),
            source.mac_coex_pti(MacCoexEvent::Event10),
        ];
        Self {
            rx_ack,
            wifi_default,
            tb,
            beamforming,
            multi_target,
        }
    }
}

pub trait MacColdCoexHardware {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti);
}

impl MacColdCoexHardware for WifiMacColdHal<'_> {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti) {
        self.initialize_coex(
            pti.rx_ack.value(),
            pti.wifi_default.value(),
            pti.tb.map(MacCoexPti::value),
            pti.beamforming.map(MacCoexPti::value),
            pti.multi_target.map(MacCoexPti::value),
        );
    }
}

#[cfg(test)]
mod tests;
