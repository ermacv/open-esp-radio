//! HIL image classes: each class's identity, the Cargo features it is
//! built with, and the keys an image of it reports on each chip.
//!
//! A scenario names the class it needs; the image builder builds a class
//! from these recipes, and the runner identifies a flashed image by the keys
//! it reports.

mod keys;

pub use keys::{
    agent_chips, classify_flashed, declares, enabled_features_on, image_keys_on, image_keys_with,
};

use oer_hil_schema::image::ImageClass;

/// The network implementation an image class links on a chip whose runtime
/// declares [`NETWORK_FEATURE`]: owned Xarxa/Embassy, the maintained
/// owner-transfer forks the manifests declare. Image records and build
/// directories name it.
pub const NETWORK: &str = "owned-xarxa";

/// The Cargo feature that selects [`NETWORK`] in an image.
pub const NETWORK_FEATURE: &str = "owned-network";

/// The network integration `class`'s image on `chip` links: [`NETWORK`]
/// where the chip's runtime declares [`NETWORK_FEATURE`] and the class links
/// a network, `None` otherwise.
pub fn network_on(class: ImageClass, chip: &str) -> Option<&'static str> {
    (declares(chip, NETWORK_FEATURE)
        && class.build_features(NETWORK_FEATURE) != class.runtime_features())
    .then_some(NETWORK)
}

/// The Cargo features `class`'s image on `chip` is built with: its own and,
/// where it links one, the network integration's.
pub fn build_features_on(class: ImageClass, chip: &str) -> String {
    match network_on(class, chip) {
        Some(_) => class.build_features(NETWORK_FEATURE),
        None => class.runtime_features().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_diagnostic_class_is_named_diagnostic() {
        for class in oer_hil_schema::image::ImageClass::ALL {
            assert_eq!(
                class.diagnostic(),
                class.id().starts_with("diagnostic-"),
                "{}",
                class.id()
            );
        }
    }
}
