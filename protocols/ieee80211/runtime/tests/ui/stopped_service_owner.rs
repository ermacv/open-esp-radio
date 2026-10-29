#![no_std]
use oer_ieee80211_port::SupportedChannel;
use oer_ieee80211_runtime::station::{PortAck, PortRxMeta, StationPort, StoppedService};
use oer_ieee80211_upper_mac::rate_selection::RateSelection;

pub fn restart_once<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id, C: oer_ieee80211_sta::machine::StaCallbacks, S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>>(
    stopped: StoppedService<'p, 'id, B, C, S>,
    channel: SupportedChannel<B::Caps>,
) {
    let restarted = stopped.restart(channel);
    #[cfg(ui_reject)]
    drop(stopped); // ui-error: E0382
    drop(restarted);
}
