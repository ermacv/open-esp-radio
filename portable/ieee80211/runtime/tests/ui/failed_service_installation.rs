#![no_std]
use oer_radio_port::PortBackend;
use oer_ieee80211_runtime::station_service::FailedInstallation;

pub fn finish<'p, 'id, B: PortBackend>(owner: FailedInstallation<'p, 'id, B>, control: B::Uninstall) {
    #[cfg(ui_reject)]
    drop(owner.enable()); // ui-error: E0599
    #[cfg(not(ui_reject))]
    drop(owner.uninstall(control, ()));
}
