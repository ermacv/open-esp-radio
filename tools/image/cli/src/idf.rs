//! `cargo fw idf`: list and build the catalog of tracked ESP-IDF firmware
//! ([`oer_image_build::esp_idf::catalog`], which builds it).

use std::path::Path;

use oer_image_build::Result;
use oer_image_build::esp_idf::catalog::{self, entries, entry};
use oer_image_esp_idf as idf;

/// `cargo fw idf list`: image, chip, project and last build.
pub fn list(root: &Path) -> Result<()> {
    for entry in entries(root)? {
        let built = std::fs::read(idf::output(root, &entry.project(root)).join("build.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<idf::Build>(&bytes).ok())
            .map_or_else(
                || String::from("not built"),
                |build| format!("built {}", &build.application_sha256[..12]),
            );
        let held = entry
            .hold
            .as_deref()
            .map_or_else(String::new, |reason| format!(" HELD: {reason}"));
        println!(
            "{:<24} {:<10} {:<48} {built}{held}",
            entry.image,
            entry.chip,
            entry.directory.display()
        );
    }
    Ok(())
}

/// `cargo fw idf build IMAGE`: build against the pinned ESP-IDF and print
/// the image, its digest and its file.
pub fn build(root: &Path, image: &str) -> Result<()> {
    let entries = entries(root)?;
    let entry = entry(&entries, image)?;
    let build = catalog::build(root, image)?;
    let (path, digest) = catalog::product(&build);
    println!("{:<24} {digest} {}", entry.image, path.display());
    Ok(())
}
