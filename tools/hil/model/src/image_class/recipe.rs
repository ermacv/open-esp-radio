//! Where each image class's runtime is built from on each chip: the
//! features each runtime manifest of this tree declares, by chip and class.
//!
//! A class names the Cargo features its runtime is built with; a chip's
//! runtime builds the class when its manifest declares every one of them.
//! What a flashed image serves is its own report (`Hello` and its image key
//! pages), never derived from these features.

use crate::schema::image::ImageClass;

// Every chip's HIL image manifest's features (`build.rs`).
include!(concat!(env!("OUT_DIR"), "/runtime_features.rs"));

/// The features `class`'s runtime manifest on `chip` declares; `None` where
/// the chip has no runtime for the class.
fn declared_features(class: ImageClass, chip: &str) -> Option<&'static [&'static str]> {
    RUNTIME_FEATURES
        .iter()
        .find_map(|(name, id, features)| (*name == chip && *id == class.id()).then_some(*features))
}

pub(super) fn declares_for(class: ImageClass, chip: &str, feature: &str) -> bool {
    declared_features(class, chip).is_some_and(|declared| declared.contains(&feature))
}

/// Whether `chip`'s runtime builds `class`: it has a manifest for the class
/// that declares every feature of the class.
pub fn builds(class: ImageClass, chip: &str) -> bool {
    declared_features(class, chip).is_some_and(|declared| {
        class
            .runtime_features()
            .split(',')
            .all(|feature| declared.contains(&feature))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chip_builds_the_classes_its_manifests_declare() {
        assert!(builds(ImageClass::DiagnosticPhy, "esp32s31"));
        assert!(builds(ImageClass::DiagnosticUsbJtagOff, "esp32s31"));
        assert!(declares_for(
            ImageClass::DiagnosticUsbJtagOff,
            "esp32s31",
            "usb-jtag-off"
        ));
        assert!(!declares_for(
            ImageClass::DiagnosticPhy,
            "esp32s31",
            "usb-jtag-off"
        ));
        assert!(!builds(ImageClass::DiagnosticPhy, "esp32c5"));
    }

    #[test]
    fn every_chip_with_an_agent_builds_a_class() {
        for (chip, _, _) in RUNTIME_FEATURES {
            assert!(
                ImageClass::ALL.iter().any(|class| builds(*class, chip)),
                "{chip} builds no class"
            );
        }
    }
}
