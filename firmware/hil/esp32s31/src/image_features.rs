//! This image's Cargo features and the keys it reports for them.

use embassy_sync::once_lock::OnceLock;
use oer_hil_image_keys::ImageKeys;

include!(concat!(env!("OUT_DIR"), "/enabled_features.rs"));

/// The keys of this image: the same function of its Cargo features the host
/// derives each image class's expected keys with.
pub(crate) fn keys() -> &'static ImageKeys {
    static KEYS: OnceLock<ImageKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        oer_hil_image_keys::image_keys(&|feature| ENABLED_FEATURES.contains(&feature))
    })
}
