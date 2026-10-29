//! The HIL services each image of this package mounts, and the key set it
//! reports: the union of the keys of those services and of the base module.

#[cfg(any(
    test,
    feature = "system-watchdog",
    feature = "system-panic-reset",
    feature = "wifi-station"
))]
use oer_hil_agent::lifecycle::Request as ResetRequest;

#[cfg(any(test, feature = "system-watchdog"))]
oer_hil_agent::requests! {
    /// The image's watchdog fault injections.
    pub enum WatchdogRequest (sessions = false) {
        Test(oer_hil_protocol::system::WatchdogTest),
    }
}

#[cfg(any(test, feature = "usb-jtag-off"))]
oer_hil_agent::requests! {
    /// The image's USB pad switch.
    pub enum UsbRequest (sessions = false) {
        Disable(oer_hil_protocol::system::DisableUsb),
    }
    publishes [oer_hil_protocol::system::UsbJtagOff]
}

/// The image entry's optional HIL panic hook, independent of its requests.
pub(crate) struct PanicHook;

impl oer_hil_agent::base::Service for PanicHook {
    const KEYS: &'static [oer_hil_protocol::Key] = &[
        #[cfg(feature = "panic-hook")]
        <oer_hil_protocol::system::PanicHook as oer_hil_protocol::Message>::KEY,
    ];
}

#[cfg(any(test, feature = "wifi-station"))]
oer_hil_agent::requests! {
    /// The station image's requests, the topics it publishes and its
    /// station property.
    pub enum StationRequests (sessions = false) {
        Start(oer_ieee80211_control::StartStation),
        Connect(oer_ieee80211_control::ConnectStation),
        Disconnect(oer_ieee80211_control::DisconnectStation),
        Scan(oer_ieee80211_control::ScanStation),
        Status(oer_ieee80211_control::GetStationStatus),
        Counters(oer_ieee80211_control::GetStationCounters),
        Stop(oer_ieee80211_control::StopStation)
    }
    publishes [
        oer_ieee80211_control::Station,
        oer_ieee80211_control::StationLink,
        oer_ieee80211_control::StationCommandFinished,
        oer_ieee80211_control::StationTransmitRefused,
        oer_ieee80211_control::StationMsduRefusal,
        oer_ieee80211_control::StationGroupKey,
        oer_ieee80211_control::StationArpFrame,
        oer_ieee80211_control::StationReceiveDrop,
        oer_ieee80211_control::StationStopped,
        oer_ieee80211_control::StationServiceEnded,
        oer_network_control::Ready,
    ]
}

#[cfg(any(test, all(feature = "system-watchdog", not(feature = "usb-jtag-off"))))]
oer_hil_agent::image_keys! {
    /// The watchdog image: watchdog tests, requested reset and panic hook.
    pub(crate) WATCHDOG_KEYS = [WatchdogRequest, ResetRequest, PanicHook]
}

#[cfg(any(test, feature = "usb-jtag-off"))]
oer_hil_agent::image_keys! {
    /// The watchdog image that also switches its USB Serial/JTAG off.
    pub(crate) USB_JTAG_OFF_KEYS = [WatchdogRequest, UsbRequest, ResetRequest, PanicHook]
}

#[cfg(any(test, feature = "system-panic-reset"))]
oer_hil_agent::image_keys! {
    /// The panic-reset image: explicit reset through the product retained path.
    pub(crate) PANIC_RESET_KEYS = [ResetRequest, PanicHook]
}

#[cfg(all(not(test), feature = "wifi-station"))]
use crate::wifi_station::udp::Requests as UdpRequests;
#[cfg(test)]
oer_hil_agent::requests! { pub enum UdpRequests(sessions = false) { Start(oer_network_control::StartDatagrams), Stop(oer_network_control::StopDatagrams), } publishes [oer_network_control::DatagramsFinished, oer_network_control::Udp] }

#[cfg(any(test, feature = "wifi-station"))]
oer_hil_agent::image_keys! {
    /// The station image.
    pub(crate) STATION_KEYS = [StationRequests, oer_hil_agent::capture::Request, ResetRequest, UdpRequests, PanicHook]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::base::ImageKeySet;
    use oer_hil_protocol::{Key, Message, base, system};
    use oer_ieee80211_control as wifi;
    use oer_network_control as network;

    fn set(keys: ImageKeySet<'_>) -> BTreeSet<Key> {
        keys.keys().iter().copied().collect()
    }

    fn expected(groups: &[&[Key]]) -> BTreeSet<Key> {
        Base::KEYS
            .iter()
            .chain(groups.iter().copied().flatten())
            .chain(PanicHook::KEYS)
            .copied()
            .collect()
    }

    const WATCHDOG: &[Key] = &[system::WatchdogTest::KEY, system::WatchdogArmed::KEY];
    const RESET: &[Key] = &[
        oer_hil_protocol::lifecycle::RequestReset::KEY,
        oer_hil_protocol::lifecycle::ResetAccepted::KEY,
    ];
    const USB: &[Key] = &[
        system::DisableUsb::KEY,
        system::UsbDisabled::KEY,
        system::UsbJtagOff::KEY,
    ];

    #[test]
    fn properties_follow_the_installed_hook_and_usb_variant() {
        for (keys, usb_off) in [
            (WATCHDOG_KEYS, false),
            (USB_JTAG_OFF_KEYS, true),
            (PANIC_RESET_KEYS, false),
            (STATION_KEYS, false),
        ] {
            assert_eq!(
                keys.contains(system::PanicHook::KEY),
                cfg!(feature = "panic-hook")
            );
            assert_eq!(keys.contains(system::UsbJtagOff::KEY), usb_off);
        }
    }

    #[test]
    fn each_image_reports_its_services_and_the_base_module() {
        assert_eq!(set(WATCHDOG_KEYS), expected(&[WATCHDOG, RESET]));
        assert_eq!(set(USB_JTAG_OFF_KEYS), expected(&[WATCHDOG, USB, RESET]));
        assert_eq!(set(PANIC_RESET_KEYS), expected(&[RESET]));
        assert_eq!(
            set(STATION_KEYS),
            expected(&[
                wifi::KEYS,
                &[base::Accepted::KEY, network::Ready::KEY],
                RESET,
                <oer_hil_agent::capture::Request as Service>::KEYS,
                UdpRequests::KEYS
            ])
        );
    }

    #[test]
    fn a_response_two_endpoints_share_is_reported_once() {
        let accepted = STATION_KEYS
            .keys()
            .iter()
            .filter(|key| **key == base::Accepted::KEY)
            .count();
        assert_eq!(accepted, 1);
        assert!(STATION_KEYS.keys().windows(2).all(|pair| pair[0] < pair[1]));
    }
}
