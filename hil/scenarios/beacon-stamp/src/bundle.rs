//! The identity of a bundle the standard `cargo fw build <image>` produced, from that build's own records in the bundle directory:
//! `sizes.json`'s provenance (package, commit, dirty), and the whole flash
//! set through the shared loading boundary every flash writer reads
//! (`oer_tools_image_bundle::ImageBundle::load` and its snapshot): every
//! flash file present, read once and matching the digest and size its
//! manifest recorded. Nothing is taken from the launching directory; a
//! bundle of another image, one missing a flash file, or one whose files
//! or records differ, is refused before the lease.

use oer_tools_image_bundle::ImageBundle;
use serde_json::Value;
use std::path::Path;

/// The probe's package.
pub const PACKAGE: &str = "oer-hil-probes-esp32s31-beacon-stamp";
/// The responder's package.
pub const RESPONDER_PACKAGE: &str = "oer-hil-probes-esp32s31-ack-responder";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub commit: String,
    pub dirty: bool,
    pub application_sha256: String,
    /// Every flash file, in write order: its description and SHA-256.
    pub flash: Vec<(&'static str, String)>,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// The identity of `bundle`, built from `package`: its provenance, and its
/// whole flash set as the writer would read it.
pub fn identity(bundle: &Path, package_wanted: &str) -> Result<Identity, String> {
    let sizes = read_json(&bundle.join("sizes.json"))?;
    let provenance = sizes
        .get("provenance")
        .ok_or("sizes.json has no provenance")?;
    let text = |name: &str| {
        provenance
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("sizes.json provenance has no {name}"))
    };
    let package = text("package")?;
    if package != package_wanted {
        return Err(format!(
            "the bundle was built from `{package}`, not `{package_wanted}`"
        ));
    }
    let commit = text("commit")?;
    let dirty = provenance
        .get("dirty")
        .and_then(Value::as_bool)
        .ok_or("sizes.json provenance has no dirty flag")?;
    let snapshot = ImageBundle::load(bundle)
        .and_then(|loaded| loaded.snapshot())
        .map_err(|error| error.to_string())?;
    let application = snapshot
        .segments
        .iter()
        .find(|segment| segment.description == "application")
        .ok_or("the bundle's flash set has no application")?;
    let sized_bytes = sizes
        .pointer("/image_files/application.bin")
        .and_then(Value::as_u64);
    if sized_bytes != Some(application.data.len() as u64) {
        return Err("bundle.json and sizes.json describe different applications".to_owned());
    }
    Ok(Identity {
        commit,
        dirty,
        application_sha256: application.sha256.clone(),
        flash: snapshot
            .segments
            .iter()
            .map(|segment| (segment.description, segment.sha256.clone()))
            .collect(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A chip with a flash map, whose bundles hold an OTA selection.
    fn profile() -> oer_tools_chip_profile::Profile {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path();
        for (path, text) in oer_chips_family_facts::tracked::FILES {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        oer_tools_chip_profile::Profile::all(root)
            .unwrap()
            .into_iter()
            .find(|profile| profile.facts.flash.is_some())
            .unwrap()
    }

    /// A bundle of `package` published in `output` by the bundle crate's
    /// own path, its application `application`, with the build's sizes.
    pub(crate) fn bundle(output: &Path, package: &str, application: &[u8]) {
        let profile = profile();
        let staging = oer_tools_image_bundle::staging_directory(output).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        let mut staged = ImageBundle::new(&staging, &profile, profile.facts.flash.clone().unwrap());
        staged.otadata = true;
        for (file, bytes) in [
            (staged.bootloader(), &b"boot"[..]),
            (staged.partitions(), b"table"),
            (staged.application(), application),
            (staged.otadata().unwrap(), b"select"),
        ] {
            std::fs::write(file, bytes).unwrap();
        }
        staged.publish(output).unwrap();
        let sizes = json!({
            "schema": 2,
            "provenance": {"commit": "834fd061621c", "dirty": false, "package": package},
            "image_files": {"application.bin": application.len()},
        });
        std::fs::write(output.join("sizes.json"), sizes.to_string()).unwrap();
    }

    fn published(package: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        bundle(directory.path(), package, b"probe");
        directory
    }

    #[test]
    fn the_identity_is_the_builds_own_record_and_its_whole_flash_set() {
        let directory = published(PACKAGE);
        let identity = identity(directory.path(), PACKAGE).unwrap();
        assert_eq!(identity.commit, "834fd061621c");
        assert!(!identity.dirty);
        let probe = oer_tools_os_durable::sha256_bytes(b"probe");
        assert_eq!(identity.application_sha256, probe);
        assert_eq!(
            identity.flash,
            [
                ("bootloader", oer_tools_os_durable::sha256_bytes(b"boot")),
                (
                    "partition table",
                    oer_tools_os_durable::sha256_bytes(b"table")
                ),
                ("application", probe),
                (
                    "OTA selection",
                    oer_tools_os_durable::sha256_bytes(b"select")
                ),
            ]
        );
    }

    #[test]
    fn a_bundle_missing_a_flash_file_is_refused() {
        for file in ["bootloader.bin", "partitions.bin", "otadata.bin"] {
            let directory = published(PACKAGE);
            std::fs::remove_file(directory.path().join(file)).unwrap();
            let error = identity(directory.path(), PACKAGE).unwrap_err();
            assert!(error.contains("lacks its"), "{file}: {error}");
        }
    }

    #[test]
    fn a_flash_file_that_differs_from_its_manifest_is_refused() {
        for file in ["bootloader.bin", "application.bin", "otadata.bin"] {
            let directory = published(PACKAGE);
            std::fs::write(directory.path().join(file), b"other").unwrap();
            let error = identity(directory.path(), PACKAGE).unwrap_err();
            assert!(
                error.contains("not the file it was published with"),
                "{file}: {error}"
            );
        }
    }

    #[test]
    fn a_bundle_of_another_image_is_refused() {
        let directory = published("oer-hil-probes-esp32s31-rx-chain");
        assert!(identity(directory.path(), PACKAGE).is_err());
    }

    #[test]
    fn records_of_different_builds_are_refused() {
        let directory = published(PACKAGE);
        let sizes = json!({
            "schema": 2,
            "provenance": {"commit": "834fd061621c", "dirty": false, "package": PACKAGE},
            "image_files": {"application.bin": 99},
        });
        std::fs::write(directory.path().join("sizes.json"), sizes.to_string()).unwrap();
        assert!(identity(directory.path(), PACKAGE).is_err());
    }

    #[test]
    fn a_directory_without_records_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        assert!(identity(directory.path(), PACKAGE).is_err());
    }
}
