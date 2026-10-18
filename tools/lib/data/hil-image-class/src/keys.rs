//! What each image class's runtime compiles and reports, derived from its
//! chip's runtime manifest.
//!
//! A class names the Cargo features its runtime is built with; the manifest
//! says which features each of those enables in turn. The closure of that
//! graph is exactly what `cfg(feature = ...)` sees in the runtime, and the
//! runtime reports its image keys with the same function of it
//! ([`oer_hil_image_keys::image_keys`]), so the host's
//! expectation of a flashed image cannot drift from the firmware. A flashed
//! image is the one class of its chip whose keys it reports.
//! Only the handlers implemented by the image contribute keys.

use std::collections::{BTreeMap, BTreeSet};

use oer_hil_protocol::DeviceImageKeys;
use oer_hil_schema::image::{FeatureDelta, ImageClass};

// Every chip's HIL agent manifest of this tree (`build.rs`).
include!(concat!(env!("OUT_DIR"), "/manifests.rs"));

/// The chips whose HIL agent this tree builds, in profile order.
pub fn agent_chips() -> impl Iterator<Item = &'static str> {
    RUNTIME_MANIFESTS.iter().map(|(chip, _)| *chip)
}

fn runtime_manifest(chip: &str) -> Option<&'static str> {
    RUNTIME_MANIFESTS
        .iter()
        .find_map(|(name, manifest)| (*name == chip).then_some(*manifest))
}

/// The runtime's features and the features each enables, leaving out
/// optional dependencies (`dep:x`) and other packages' features (`x/f`,
/// `x?/f`), which `cfg(feature)` of the runtime does not see.
fn feature_graph(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let manifest: toml::Table = toml::from_str(manifest).expect("the runtime manifest parses");
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("the runtime manifest declares features");
    features
        .iter()
        .map(|(name, enables)| {
            let own = enables
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .filter(|member| !member.starts_with("dep:") && !member.contains('/'))
                .map(str::to_owned)
                .collect();
            (name.clone(), own)
        })
        .collect()
}

/// The features `roots` enable through `graph`, themselves included.
fn closure(graph: &BTreeMap<String, Vec<String>>, roots: &[&str]) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    let mut pending = roots
        .iter()
        .map(|root| (*root).to_owned())
        .collect::<Vec<_>>();
    while let Some(feature) = pending.pop() {
        if enabled.insert(feature.clone())
            && let Some(members) = graph.get(&feature)
        {
            pending.extend(members.iter().cloned());
        }
    }
    enabled
}

/// Whether `chip`'s runtime declares the Cargo feature `feature`.
pub fn declares(chip: &str, feature: &str) -> bool {
    runtime_manifest(chip).is_some_and(|manifest| feature_graph(manifest).contains_key(feature))
}

/// Every Cargo feature `class`'s runtime on `chip` is built with, or `None`
/// where the chip's runtime has no such class.
pub fn enabled_features_on(class: ImageClass, chip: &str) -> Option<BTreeSet<String>> {
    enabled_features_with(class, chip, &FeatureDelta::default())
}

/// [`enabled_features_on`] for `class`'s features with an experiment's
/// `delta` applied.
fn enabled_features_with(
    class: ImageClass,
    chip: &str,
    delta: &FeatureDelta,
) -> Option<BTreeSet<String>> {
    let graph = feature_graph(runtime_manifest(chip)?);
    // A class whose own features the chip's runtime lacks is not built
    // there. The network integration's feature gates no key, so the
    // class's own features decide what it reports.
    let own = delta.apply(class.runtime_features());
    let own = own.as_str();
    let roots: Vec<&str> = own.split(',').filter(|root| !root.is_empty()).collect();
    if !own.split(',').all(|feature| graph.contains_key(feature)) {
        return None;
    }
    let roots: Vec<&str> = roots
        .into_iter()
        .filter(|root| graph.contains_key(*root))
        .collect();
    Some(closure(&graph, &roots))
}

/// The image keys `class`'s runtime on `chip` reports, or `None` for the
/// boot smoke image, which reports none, and for a class the chip does not
/// build.
pub fn image_keys_on(class: ImageClass, chip: &str) -> Option<DeviceImageKeys> {
    image_keys_with(class, chip, &FeatureDelta::default())
}

/// The image keys `class`'s runtime on `chip` reports when an A/B
/// experiment builds it with `delta` applied to its features: an arm
/// without an instrument reports fewer keys than its class.
pub fn image_keys_with(
    class: ImageClass,
    chip: &str,
    delta: &FeatureDelta,
) -> Option<DeviceImageKeys> {
    if class == ImageClass::BootSmoke {
        return None;
    }
    let enabled = enabled_features_with(class, chip, delta)?;
    Some(DeviceImageKeys::of_keys(oer_hil_image_keys::image_keys(
        &|feature| enabled.contains(feature),
    )))
}

/// The class of `chip` whose keys a flashed image reports.
pub fn classify_flashed(chip: &str, image_keys: &DeviceImageKeys) -> Option<ImageClass> {
    ImageClass::ALL.into_iter().find(|class| {
        image_keys_on(*class, chip).is_some_and(|expected| expected.same_keys(image_keys))
    })
}

#[cfg(test)]
mod tests {
    use oer_hil_protocol::{Message, base, network, system};

    use super::*;

    /// The chip whose agent builds the most classes: the tree's full HIL
    /// agent.
    fn full_agent() -> &'static str {
        RUNTIME_MANIFESTS
            .iter()
            .map(|(chip, _)| *chip)
            .max_by_key(|chip| {
                ImageClass::ALL
                    .iter()
                    .filter(|class| image_keys_on(**class, chip).is_some())
                    .count()
            })
            .expect("a chip has a HIL agent")
    }

    fn keys(class: ImageClass) -> DeviceImageKeys {
        image_keys_on(class, full_agent()).unwrap()
    }

    #[test]
    fn the_feature_graph_keeps_only_the_runtime_s_own_features() {
        let graph = feature_graph(
            r#"
            [features]
            a = ["b", "dep:x", "y/z", "w?/v"]
            b = ["c"]
            c = []
            "#,
        );
        assert_eq!(graph["a"], ["b"]);
        assert_eq!(
            closure(&graph, &["a"]),
            ["a", "b", "c"].map(String::from).into()
        );
    }

    #[test]
    fn every_class_of_a_chip_reports_its_own_keys_and_classifies_as_itself() {
        for (chip, _) in RUNTIME_MANIFESTS {
            let mut seen = BTreeMap::new();
            for class in ImageClass::ALL {
                let Some(image_keys) = image_keys_on(class, chip) else {
                    continue;
                };
                if let Some(other) = seen.insert(image_keys.keys().clone(), class) {
                    panic!(
                        "{chip}: {} and {} report the same keys",
                        other.id(),
                        class.id()
                    );
                }
                assert_eq!(
                    classify_flashed(chip, &image_keys),
                    Some(class),
                    "{chip}: {}",
                    class.id()
                );
            }
            assert!(!seen.is_empty(), "{chip} builds no class");
        }
    }

    #[test]
    fn a_key_set_no_class_builds_is_no_class() {
        let watchdog = keys(ImageClass::SystemWatchdog);
        let mut foreign = watchdog.keys().clone();
        foreign.insert(<network::Udp as Message>::KEY);
        assert_eq!(
            classify_flashed(full_agent(), &DeviceImageKeys::of_keys(foreign)),
            None
        );
        let mut missing = watchdog.keys().clone();
        assert!(missing.remove(&<base::Hello as Message>::KEY));
        assert_eq!(
            classify_flashed(full_agent(), &DeviceImageKeys::of_keys(missing)),
            None
        );
    }

    #[test]
    fn every_feature_the_keys_read_is_the_runtime_s() {
        let graph = feature_graph(runtime_manifest(full_agent()).unwrap());
        for feature in oer_hil_image_keys::READ_FEATURES {
            assert!(graph.contains_key(*feature), "{feature}");
        }
    }

    #[test]
    fn the_classes_report_their_active_handlers() {
        let watchdog = keys(ImageClass::SystemWatchdog);
        assert!(watchdog.has::<system::WatchdogTest>() && !watchdog.has::<system::DisableUsb>());
        assert!(!watchdog.has::<network::Udp>());
        let usb_off = keys(ImageClass::DiagnosticUsbJtagOff);
        assert!(usb_off.has::<system::WatchdogTest>() && usb_off.has::<system::DisableUsb>());
        let panic = keys(ImageClass::SystemPanicReset);
        assert!(panic.has::<system::InjectPanic>() && !panic.has::<system::WatchdogTest>());
    }

    #[test]
    fn a_partial_agent_builds_only_its_own_classes() {
        for (chip, _) in RUNTIME_MANIFESTS {
            if *chip == full_agent() {
                continue;
            }
            let built = ImageClass::ALL
                .iter()
                .filter(|class| image_keys_on(**class, chip).is_some())
                .count();
            assert!(built > 0 && built < ImageClass::ALL.len(), "{chip}");
        }
    }

    #[test]
    fn an_arm_without_an_instrument_reports_its_class_s_keys_less_the_instrument_s() {
        let chip = full_agent();
        let without: FeatureDelta = "-usb-jtag-off".parse().unwrap();
        let class = keys(ImageClass::DiagnosticUsbJtagOff);
        let arm = image_keys_with(ImageClass::DiagnosticUsbJtagOff, chip, &without).unwrap();
        assert!(class.has::<system::DisableUsb>() && !arm.has::<system::DisableUsb>());
        assert!(arm.has::<system::WatchdogTest>());
        // Less the instrument, the arm reports another class's keys.
        assert_eq!(
            classify_flashed(chip, &arm),
            Some(ImageClass::SystemWatchdog)
        );
        assert!(
            image_keys_with(
                ImageClass::DiagnosticUsbJtagOff,
                chip,
                &FeatureDelta::default()
            )
            .unwrap()
            .same_keys(&class)
        );
    }
}
