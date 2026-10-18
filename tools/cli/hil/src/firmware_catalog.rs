//! `cargo hil firmware`: list and build the catalog of tracked ESP-IDF
//! firmware ([`oer_image::esp_idf::catalog`], which builds it); `cargo fw
//! flash` writes one to a board, HIL runs through the flash operation.
use crate::Result;
use oer_esp_idf as idf;
use oer_image::esp_idf::catalog::{self, entries, entry};
use oer_process::Checkout;

/// `cargo hil firmware list`: image, chip, project and last build.
pub fn list(ctx: &Checkout) -> Result<()> {
    for entry in entries(&ctx.root)? {
        let built =
            std::fs::read(idf::output(&ctx.root, &entry.project(&ctx.root)).join("build.json"))
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

/// `cargo hil firmware build IMAGE`: build against the pinned ESP-IDF and
/// print the image, its digest and its file.
pub fn build(ctx: &Checkout, image: &str) -> Result<idf::Build> {
    let entries = entries(&ctx.root)?;
    let entry = entry(&entries, image)?;
    let build = catalog::build(&ctx.root, image)?;
    let (path, digest) = catalog::product(&build);
    println!("{:<24} {digest} {}", entry.image, path.display());
    Ok(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_its_peers() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let entries = entries(&root).unwrap();
        assert!(entries.iter().any(|entry| entry.image == "ieee802154-peer"));
    }
}
