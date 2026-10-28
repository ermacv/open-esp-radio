#![forbid(unsafe_code)]
//! The keys a HIL image reports, as a function of the Cargo
//! features it was built with.
//!
//! The runtime pages this set out for `base/image-keys/get`, and the host
//! derives the set each image class reports from the same function applied
//! to the class's features and the runtime manifest's feature graph. The two
//! cannot drift apart: the host identifies a flashed image by this set.
//!
//! The set holds the key of every endpoint the image serves, of every message
//! it sends (an endpoint's response changes its key without changing the
//! request's) and of every property it has.

use crate::{Key, Message, base, system};

/// The most keys one image reports.
pub const IMAGE_KEYS: usize = 80;

/// An immutable, sorted and distinct capability set, bounded by [`IMAGE_KEYS`].
/// Only the feature catalogue constructs this owner.
///
/// ```compile_fail
/// let mut keys = oer_hil_protocol::feature_keys::image_keys(&|_| true);
/// keys.0.clear();
/// ```
#[derive(Clone, Debug)]
pub struct ImageKeys(heapless::Vec<Key, IMAGE_KEYS>);

impl ImageKeys {
    pub fn as_slice(&self) -> &[Key] {
        &self.0
    }

    pub fn as_set(&self) -> base::ImageKeySet<'_> {
        self.into()
    }
}

impl IntoIterator for ImageKeys {
    type Item = Key;
    type IntoIter = <heapless::Vec<Key, IMAGE_KEYS> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

const BASE: [Key; 11] = [
    base::Hello::KEY,
    base::GetHello::KEY,
    base::GetImageKeys::KEY,
    base::ImageKeyPage::KEY,
    base::Rejected::KEY,
    base::GetBootStatus::KEY,
    base::BootEvidence::KEY,
    base::GetPostMortemCheckpoints::KEY,
    base::PostMortemCheckpoints::KEY,
    base::GetLinkHealth::KEY,
    base::LinkHealth::KEY,
];

#[cfg(feature = "wifi")]
const WIFI: &[Key] = &[
    crate::wifi::Station::KEY,
    crate::wifi::StartStation::KEY,
    crate::wifi::StationLink::KEY,
    crate::wifi::ProbeStation::KEY,
    crate::wifi::StationProbeFinished::KEY,
    crate::wifi::StationProbeReady::KEY,
    crate::wifi::GetStationCounters::KEY,
    crate::wifi::StationCounters::KEY,
    crate::wifi::StationGroupKey::KEY,
    crate::wifi::StationArpFrame::KEY,
    crate::wifi::StationReceiveDrop::KEY,
    crate::wifi::StopStation::KEY,
    crate::wifi::StationStopped::KEY,
    crate::wifi::StationServiceEnded::KEY,
    base::Accepted::KEY,
    #[cfg(feature = "network")]
    crate::network::Ready::KEY,
];

#[cfg(feature = "phy")]
const PHY: &[Key] = &[
    crate::phy::StartTrackingHold::KEY,
    crate::phy::HoldStarted::KEY,
    crate::phy::GetTrackingHold::KEY,
    crate::phy::TrackingHold::KEY,
    crate::phy::StopTrackingHold::KEY,
    crate::phy::HoldStopped::KEY,
    crate::phy::GetCalibrationCacheIdentity::KEY,
    crate::phy::CalibrationCacheIdentity::KEY,
    crate::phy::StartCalibration::KEY,
    crate::phy::CalibrationStarted::KEY,
    crate::phy::WriteCalibrationCachePage::KEY,
    crate::phy::CalibrationCachePageWritten::KEY,
    crate::phy::CompleteCalibrationCache::KEY,
    crate::phy::CalibrationCacheCompleted::KEY,
    crate::phy::GetCalibrationStartStatus::KEY,
    crate::phy::CalibrationStartStatus::KEY,
    crate::phy::ReadCalibrationSnapshot::KEY,
    crate::phy::CalibrationSnapshot::KEY,
    crate::phy::HostCalibrationCache::KEY,
    crate::phy::RestartCaptureRadio::KEY,
    crate::phy::CaptureRestarted::KEY,
    crate::phy::StopCapture::KEY,
    crate::phy::CaptureStopped::KEY,
    crate::phy::ReadCalibrationProjection::KEY,
    crate::phy::CalibrationProjection::KEY,
    crate::phy::ReadRegisterImage::KEY,
    crate::phy::RegisterImage::KEY,
    crate::phy::ReadAnalogImage::KEY,
    crate::phy::AnalogImage::KEY,
];

#[cfg(feature = "lower-mac")]
const LOWER_MAC: &[Key] = &[
    crate::lower_mac::Prepare::KEY,
    crate::lower_mac::Prepared::KEY,
    crate::lower_mac::RunCase::KEY,
    crate::lower_mac::CaseTrace::KEY,
];

const PANIC: &[Key] = &[system::InjectPanic::KEY, system::PanicInjected::KEY];
const WATCHDOG: &[Key] = &[system::WatchdogTest::KEY, system::WatchdogArmed::KEY];
const USB: &[Key] = &[system::DisableUsb::KEY, system::UsbDisabled::KEY];

macro_rules! catalogue {
    ($name:ident = [$($group:expr),* $(,)?]) => {
        const $name: [Key; 0 $(+ $group.len())*] = {
            let mut result = [Key([0; 8]); 0 $(+ $group.len())*];
            let mut destination = result.as_mut_slice();
            $(
                let mut source: &[Key] = $group;
                while let Some((key, tail)) = source.split_first() {
                    let Some((slot, remaining)) = destination.split_first_mut() else {
                        panic!("the catalogue length must hold every key");
                    };
                    *slot = *key;
                    destination = remaining;
                    source = tail;
                }
            )*
            let result = base::sorted_keys(result);
            if base::ImageKeySet::new(&result).is_err() {
                panic!("the feature catalogue must be sorted, distinct and wire-bounded");
            }
            result
        };
    };
}

catalogue!(SYSTEM_KEYS = [&BASE, PANIC, WATCHDOG, USB]);

const NORMAL_GROUPS: &[&[Key]] = &[
    &BASE,
    #[cfg(feature = "wifi")]
    WIFI,
    #[cfg(feature = "phy")]
    PHY,
    #[cfg(feature = "lower-mac")]
    LOWER_MAC,
    #[cfg(feature = "health")]
    crate::health::KEYS,
];
const NORMAL_LENGTH: usize = {
    let mut count = 0;
    let mut groups = NORMAL_GROUPS;
    while let Some((group, rest)) = groups.split_first() {
        count += group.len();
        groups = rest;
    }
    count
};
const NORMAL_KEYS: [Key; NORMAL_LENGTH] = {
    let mut keys = [Key([0; 8]); NORMAL_LENGTH];
    let mut destination = keys.as_mut_slice();
    let mut groups = NORMAL_GROUPS;
    while let Some((group, rest)) = groups.split_first() {
        let mut source = *group;
        while let Some((key, tail)) = source.split_first() {
            let Some((slot, remaining)) = destination.split_first_mut() else {
                panic!("the catalogue length must hold every key");
            };
            *slot = *key;
            destination = remaining;
            source = tail;
        }
        groups = rest;
    }
    let keys = base::sorted_keys(keys);
    if base::ImageKeySet::new(&keys).is_err() {
        panic!("the feature catalogue must be sorted, distinct and wire-bounded");
    }
    keys
};

/// The keys of a runtime image whose Cargo features are those for which
/// `enabled` is true. Catalogue geometry is checked at compile time.
pub fn image_keys(enabled: &dyn Fn(&str) -> bool) -> ImageKeys {
    let mut keys;
    if enabled("system-watchdog") {
        let disable_usb = enabled("usb-jtag-off");
        let usb_disabled = enabled("usb-jtag-off");
        keys = heapless::Vec::from_array(SYSTEM_KEYS);
        keys.retain(|key| {
            BASE.contains(key)
                || PANIC.contains(key)
                || WATCHDOG.contains(key)
                || (*key == system::DisableUsb::KEY && disable_usb)
                || (*key == system::UsbDisabled::KEY && usb_disabled)
        });
    } else if enabled("system-panic-reset") {
        keys = heapless::Vec::from_array(SYSTEM_KEYS);
        keys.retain(|key| BASE.contains(key) || PANIC.contains(key));
    } else {
        #[cfg(feature = "wifi")]
        let wifi = enabled("wifi-station");
        #[cfg(feature = "phy")]
        let phy = enabled("diagnostic-phy");
        #[cfg(feature = "lower-mac")]
        let lower_mac = enabled("diagnostic-lower-mac");
        #[cfg(feature = "health")]
        let health = enabled("diagnostic-radio-health");
        keys = heapless::Vec::from_array(NORMAL_KEYS);
        keys.retain(|key| {
            if BASE.contains(key) {
                return true;
            }
            #[cfg(feature = "wifi")]
            if WIFI.contains(key) {
                return wifi;
            }
            #[cfg(feature = "phy")]
            if PHY.contains(key) {
                return phy;
            }
            #[cfg(feature = "lower-mac")]
            if LOWER_MAC.contains(key) {
                return lower_mac;
            }
            #[cfg(feature = "health")]
            if crate::health::KEYS.contains(key) {
                return health;
            }
            false
        });
    }
    ImageKeys(keys)
}

/// Every Cargo feature [`image_keys`] reads, so the host can check that the
/// runtime manifest declares each of them.
pub const READ_FEATURES: &[&str] = &[
    "wifi-station",
    "system-watchdog",
    "system-panic-reset",
    "usb-jtag-off",
    "diagnostic-phy",
    "diagnostic-lower-mac",
    "diagnostic-radio-health",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network;

    fn with(features: &[&str]) -> ImageKeys {
        image_keys(&|feature| features.contains(&feature))
    }

    fn has<M: Message>(keys: &ImageKeys) -> bool {
        keys.as_slice().contains(&M::KEY)
    }

    #[test]
    fn every_feature_combination_is_bounded_sorted_and_distinct() {
        for mask in 0..(1_usize << READ_FEATURES.len()) {
            let keys = image_keys(&|feature| {
                READ_FEATURES
                    .iter()
                    .enumerate()
                    .any(|(index, candidate)| *candidate == feature && mask & (1 << index) != 0)
            });
            let checked = base::ImageKeySet::new(keys.as_slice()).unwrap();
            assert!(keys.as_slice().len() <= IMAGE_KEYS);
            assert_eq!(checked.hello(0), keys.as_set().hello(0));
        }
        let all_normal = image_keys(&|feature| !feature.starts_with("system-"));
        assert_eq!(all_normal.as_slice(), NORMAL_KEYS.as_slice());
    }

    #[test]
    fn the_family_follows_the_image_s_features() {
        let watchdog = with(&["system-watchdog"]);
        assert!(has::<system::WatchdogTest>(&watchdog) && !has::<network::Udp>(&watchdog));
        assert!(
            has::<system::InjectPanic>(&watchdog),
            "the watchdog image's panic hook"
        );
        let panic = with(&["system-panic-reset"]);
        assert!(has::<system::InjectPanic>(&panic) && !has::<system::WatchdogTest>(&panic));
        assert!(!has::<system::DisableUsb>(&watchdog));
        let usb_off = with(&["system-watchdog", "usb-jtag-off"]);
        assert!(has::<system::DisableUsb>(&usb_off) && has::<system::WatchdogTest>(&usb_off));
    }

    #[cfg(feature = "wifi")]
    #[test]
    fn station_image_reports_only_implemented_station_and_network_handlers() {
        let station = with(&["wifi-station"]);
        assert!(has::<crate::wifi::Station>(&station));
        assert!(has::<crate::wifi::StartStation>(&station));
        assert!(has::<crate::wifi::StationLink>(&station));
        assert!(has::<base::Accepted>(&station));
        #[cfg(feature = "network")]
        assert!(has::<network::Ready>(&station));
        assert!(!has::<network::Udp>(&station));
        assert!(!has::<system::WatchdogTest>(&station));
    }

    #[test]
    fn every_image_serves_the_base_module() {
        for features in [
            &["system-watchdog"][..],
            &["system-panic-reset"],
            &["boot-smoke"],
        ] {
            let keys = with(features);
            assert!(
                BASE.iter().all(|key| keys.as_slice().contains(key)),
                "{features:?}"
            );
        }
    }

    #[test]
    fn the_set_is_sorted_and_distinct_so_its_pages_are_stable() {
        let all = with(READ_FEATURES);
        assert!(all.as_slice().windows(2).all(|pair| pair[0] < pair[1]));
        base::ImageKeySet::new(all.as_slice()).unwrap();
    }
}
