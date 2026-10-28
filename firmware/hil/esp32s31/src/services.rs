//! The HIL services each image of this package mounts, and the key set it
//! reports: the union of the keys of those services and of the base module.

#[cfg(any(test, feature = "system-watchdog", feature = "system-panic-reset"))]
use oer_hil_agent::base::PanicRequest;
#[cfg(any(test, feature = "usb-jtag-off"))]
use oer_hil_agent::base::UsbRequest;
#[cfg(any(test, feature = "system-watchdog"))]
use oer_hil_agent::base::WatchdogRequest;

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
    /// The watchdog image: watchdog tests and the panic hook's injection.
    pub(crate) WATCHDOG_KEYS = [WatchdogRequest, PanicRequest]
}

#[cfg(any(test, feature = "usb-jtag-off"))]
oer_hil_agent::image_keys! {
    /// The watchdog image that also switches its USB Serial/JTAG off.
    pub(crate) USB_JTAG_OFF_KEYS = [WatchdogRequest, UsbRequest, PanicRequest]
}

#[cfg(any(test, feature = "system-panic-reset"))]
oer_hil_agent::image_keys! {
    /// The panic-reset image: injection through the product panic entry.
    pub(crate) PANIC_RESET_KEYS = [PanicRequest]
}

#[cfg(any(test, feature = "wifi-station"))]
oer_hil_agent::image_keys! {
    /// The station image.
    pub(crate) STATION_KEYS = [StationRequests]
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
            .copied()
            .collect()
    }

    const WATCHDOG: &[Key] = &[system::WatchdogTest::KEY, system::WatchdogArmed::KEY];
    const PANIC: &[Key] = &[system::InjectPanic::KEY, system::PanicInjected::KEY];
    const USB: &[Key] = &[system::DisableUsb::KEY, system::UsbDisabled::KEY];

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
