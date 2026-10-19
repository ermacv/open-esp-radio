//! The platform values the Wi-Fi MAC cold start reads, answered by the radio
//! system.

use oer_esp32s31_ieee80211_mac::init::{
    MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacDelayEntropy, MacSlowClockCalibration,
    MacSlowClockCalibrationSource, MacTxPowerPair, MacTxPowerSource,
};
use oer_esp32s31_phy::PhyTxTargetPowerProfile;

/// The MAC cold start's queries, in `MacColdCoexPti::query` order.
pub(crate) const MAC_COEX_EVENTS: [MacCoexEvent; 4] = [
    MacCoexEvent::Event1,
    MacCoexEvent::Event3,
    MacCoexEvent::Event10,
    MacCoexEvent::Event15,
];

/// The platform values the MAC cold start reads, all held by value: the
/// domain's coexistence priorities, the calibrated TX power profile and the
/// client's slow-clock calibration and delay entropy.
pub(crate) struct MacSources {
    pub(crate) pti: [MacCoexPti; 4],
    pub(crate) tx_power: PhyTxTargetPowerProfile,
    pub(crate) slow_clock: MacSlowClockCalibration,
    pub(crate) delay_random: u32,
}

impl MacCoexPtiSource for MacSources {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        match event {
            MacCoexEvent::Event1 => self.pti[0],
            MacCoexEvent::Event3 => self.pti[1],
            MacCoexEvent::Event10 => self.pti[2],
            MacCoexEvent::Event15 => self.pti[3],
        }
    }
}

impl MacDelayEntropy for MacSources {
    fn mac_delay_random(&mut self) -> u32 {
        self.delay_random
    }
}

impl MacSlowClockCalibrationSource for MacSources {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        self.slow_clock
    }
}

impl MacTxPowerSource for MacSources {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let pair = self.tx_power.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn probe_priority(
    table: oer_espressif_coex::CoexPtiTable,
) -> oer_esp32s31_radio_client::wifi::ProbePriority {
    use oer_espressif_coex::{wifi_event, wifi_events};
    use oer_ieee80211_lower_mac::{ConnectionFrame, Operation, TxOperation};
    let event = wifi_event(Operation::Transmit(TxOperation::Connection(
        ConnectionFrame::ProbeRequest,
    )))
    .expect("the probe operation has a coexistence event");
    let packet = table.pti(event).value();
    let scheduler = packet.min(table.pti(wifi_events::SLICE).value());
    oer_esp32s31_radio_client::wifi::ProbePriority {
        packet: MacCoexPti::new(packet).expect("the table has four-bit PTI"),
        scheduler: MacCoexPti::new(scheduler).expect("the table has four-bit PTI"),
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn probe_priority_follows_the_live_shared_operation_and_slice_entries() {
        use oer_espressif_coex::{CoexPti, CoexPtiTable, wifi_event, wifi_events};
        use oer_ieee80211_lower_mac::{ConnectionFrame, Operation, TxOperation};
        let event = wifi_event(Operation::Transmit(TxOperation::Connection(
            ConnectionFrame::ProbeRequest,
        )))
        .expect("probe event");
        let mut table = CoexPtiTable::VENDOR;
        table.set(event, CoexPti::new(2).expect("PTI"));
        table.set(wifi_events::SLICE, CoexPti::new(3).expect("PTI"));
        let priority = probe_priority(table);
        assert_eq!(
            (priority.packet.value(), priority.scheduler.value()),
            (2, 2)
        );
        table.set(event, CoexPti::new(9).expect("PTI"));
        let priority = probe_priority(table);
        assert_eq!(
            (priority.packet.value(), priority.scheduler.value()),
            (9, 3)
        );
    }
}
