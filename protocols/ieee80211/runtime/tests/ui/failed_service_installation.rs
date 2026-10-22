#![no_std]
use oer_ieee80211_runtime::station_service::FailedInstallation;

pub fn finish<'p, 'id, B: oer_ieee80211_runtime::station_service::StationPort<SlotId: 'id, Backend: 'id> + 'id>(owner: FailedInstallation<'p, 'id, B>, control: <B::Backend as oer_radio_port::PortBackend<B::SlotId>>::Uninstall) {
    #[cfg(ui_reject)]
    drop(owner.enable()); // ui-error: E0599
    #[cfg(not(ui_reject))]
    drop(owner.uninstall(control, ()));
}
