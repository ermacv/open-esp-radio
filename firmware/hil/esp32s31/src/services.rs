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
    use super::*;
    use oer_hil_protocol::base::ImageKeySet;

    fn old(features: &[&str]) -> oer_hil_protocol::feature_keys::ImageKeys {
        oer_hil_protocol::feature_keys::image_keys(&|feature| features.contains(&feature))
    }

    fn same(new: ImageKeySet<'_>, features: &[&str]) {
        let old = old(features);
        assert_eq!(new.hello(0), old.as_set().hello(0), "{features:?}");
        assert_eq!(new.page(0), old.as_set().page(0), "{features:?}");
        assert_eq!(new.page(48), old.as_set().page(48), "{features:?}");
    }

    #[test]
    fn each_image_reports_the_keys_its_features_reported() {
        same(WATCHDOG_KEYS, &["system-watchdog", "panic-hook"]);
        same(
            USB_JTAG_OFF_KEYS,
            &["system-watchdog", "usb-jtag-off", "panic-hook"],
        );
        same(PANIC_RESET_KEYS, &["system-panic-reset"]);
        same(STATION_KEYS, &["wifi-station", "panic-hook"]);
    }
}
