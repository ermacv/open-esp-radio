//! The HIL services each image of this package mounts, and the key set it
//! reports: the union of the keys of those services and of the base module.

#[cfg(any(test, feature = "system-watchdog", feature = "system-panic-reset"))]
use oer_hil_agent::base::PanicRequest;
#[cfg(any(test, feature = "usb-jtag-off"))]
use oer_hil_agent::base::UsbRequest;
#[cfg(any(test, feature = "system-watchdog"))]
use oer_hil_agent::base::WatchdogRequest;

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
        Start(oer_hil_protocol::wifi::StartStation),
        Probe(oer_hil_protocol::wifi::ProbeStation),
        Counters(oer_hil_protocol::wifi::GetStationCounters),
        Stop(oer_hil_protocol::wifi::StopStation)
    }
    publishes [
        oer_hil_protocol::wifi::Station,
        oer_hil_protocol::wifi::StationLink,
        oer_hil_protocol::wifi::StationProbeFinished,
        oer_hil_protocol::wifi::StationProbeReady,
        oer_hil_protocol::wifi::StationGroupKey,
        oer_hil_protocol::wifi::StationArpFrame,
        oer_hil_protocol::wifi::StationReceiveDrop,
        oer_hil_protocol::wifi::StationStopped,
        oer_hil_protocol::wifi::StationServiceEnded,
        oer_hil_protocol::network::Ready,
    ]
}

#[cfg(any(test, all(feature = "system-watchdog", not(feature = "usb-jtag-off"))))]
oer_hil_agent::image_keys! {
    /// The watchdog image: watchdog tests, requested reset and panic hook.
    pub(crate) WATCHDOG_KEYS = [WatchdogRequest, PanicRequest, PanicHook]
}

#[cfg(any(test, feature = "usb-jtag-off"))]
oer_hil_agent::image_keys! {
    /// The watchdog image that also switches its USB Serial/JTAG off.
    pub(crate) USB_JTAG_OFF_KEYS = [WatchdogRequest, UsbRequest, PanicRequest, PanicHook]
}

#[cfg(any(test, feature = "system-panic-reset"))]
oer_hil_agent::image_keys! {
    /// The panic-reset image: explicit reset through the product retained path.
    pub(crate) PANIC_RESET_KEYS = [PanicRequest, PanicHook]
}

#[cfg(any(test, feature = "wifi-station"))]
oer_hil_agent::image_keys! {
    /// The station image.
    pub(crate) STATION_KEYS = [StationRequests, PanicHook]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::base::ImageKeySet;
    use oer_hil_protocol::{Key, Message, base, network, system, wifi};

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
    const PANIC: &[Key] = &[system::InjectPanic::KEY, system::PanicInjected::KEY];
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
        assert_eq!(set(WATCHDOG_KEYS), expected(&[WATCHDOG, PANIC]));
        assert_eq!(set(USB_JTAG_OFF_KEYS), expected(&[WATCHDOG, USB, PANIC]));
        assert_eq!(set(PANIC_RESET_KEYS), expected(&[PANIC]));
        assert_eq!(
            set(STATION_KEYS),
            expected(&[wifi::KEYS, &[base::Accepted::KEY, network::Ready::KEY],])
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
