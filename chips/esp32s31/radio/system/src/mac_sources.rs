//! The Probe Request's coexistence priorities the radio system answers with
//! a tracking or a channel.

pub(crate) fn probe_priority(
    table: oer_chips_family_radio_coex::CoexPtiTable,
) -> oer_chips_esp32s31_radio_client::wifi::ProbePriority {
    use oer_chips_family_radio_coex::wifi_events;
    let packet = table.pti(wifi_events::PROBE_REQUEST);
    let scheduler = packet.min(table.pti(wifi_events::SLICE));
    oer_chips_esp32s31_radio_client::wifi::ProbePriority {
        packet: packet.into(),
        scheduler: scheduler.into(),
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn probe_priority_follows_the_live_shared_operation_and_slice_entries() {
        use oer_chips_family_radio_coex::{CoexPti, CoexPtiTable, wifi_events};
        let event = wifi_events::PROBE_REQUEST;
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
