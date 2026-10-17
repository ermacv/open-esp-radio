#![no_std]
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf;

pub fn snapshots(owner: &mut RadioRuntimeOwner, ap: &AccessPointTsf) -> (u64, u64) {
    let mut sta = owner.wifi_mac_hal();
    #[cfg(not(ui_reject))]
    let station = sta.station_tsf();
    #[cfg(not(ui_reject))]
    drop(sta);
    let mut second = owner.wifi_mac_hal(); // ui-error: E0499
    #[cfg(ui_reject)]
    let station = sta.station_tsf();
    (station, ap.read(&mut second).as_micros())
}
