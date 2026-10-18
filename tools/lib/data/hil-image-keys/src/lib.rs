#![no_std]
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

use oer_hil_protocol::{Key, Message, base, system};

/// The most keys one image reports.
pub const IMAGE_KEYS: usize = 80;

/// An image's keys, in the ascending order capability pages require.
pub type ImageKeys = heapless::Vec<Key, IMAGE_KEYS>;

/// What every image serves: discovery, rejection, boot and link health.
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

/// The keys of a runtime image whose Cargo features are those for which
/// `enabled` is true.
pub fn image_keys(enabled: &dyn Fn(&str) -> bool) -> ImageKeys {
    let mut keys = ImageKeys::new();
    let mut add = |key: Key, present: bool| {
        if present {
            keys.push(key)
                .expect("an image reports at most IMAGE_KEYS keys");
        }
    };
    for key in BASE {
        add(key, true);
    }
    if enabled("system-watchdog") {
        add(system::WatchdogTest::KEY, true);
        add(system::WatchdogArmed::KEY, true);
        add(system::DisableUsb::KEY, enabled("usb-jtag-off"));
        add(system::UsbDisabled::KEY, enabled("usb-jtag-off"));
        return sorted(keys);
    }
    if enabled("system-panic-reset") {
        add(system::InjectPanic::KEY, true);
        add(system::PanicInjected::KEY, true);
        return sorted(keys);
    }
    sorted(keys)
}

fn sorted(mut keys: ImageKeys) -> ImageKeys {
    keys.sort_unstable();
    keys
}

/// Every Cargo feature [`image_keys`] reads, so the host can check that the
/// runtime manifest declares each of them.
pub const READ_FEATURES: &[&str] = &["system-watchdog", "system-panic-reset", "usb-jtag-off"];

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::network;

    fn with(features: &[&str]) -> ImageKeys {
        image_keys(&|feature| features.contains(&feature))
    }

    fn has<M: Message>(keys: &ImageKeys) -> bool {
        keys.contains(&M::KEY)
    }

    #[test]
    fn the_family_follows_the_image_s_features() {
        let watchdog = with(&["system-watchdog"]);
        assert!(has::<system::WatchdogTest>(&watchdog) && !has::<network::Udp>(&watchdog));
        let panic = with(&["system-panic-reset"]);
        assert!(has::<system::InjectPanic>(&panic) && !has::<system::WatchdogTest>(&panic));
        assert!(!has::<system::DisableUsb>(&watchdog));
        let usb_off = with(&["system-watchdog", "usb-jtag-off"]);
        assert!(has::<system::DisableUsb>(&usb_off) && has::<system::WatchdogTest>(&usb_off));
    }

    #[test]
    fn every_image_serves_the_base_module() {
        for features in [
            &["system-watchdog"][..],
            &["system-panic-reset"],
            &["boot-smoke"],
        ] {
            let keys = with(features);
            assert!(BASE.iter().all(|key| keys.contains(key)), "{features:?}");
        }
    }

    #[test]
    fn the_set_is_sorted_and_distinct_so_its_pages_are_stable() {
        let all = with(READ_FEATURES);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        base::ImageKeySet::new(&all);
    }
}
